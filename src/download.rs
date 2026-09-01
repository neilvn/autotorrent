use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use regex::Regex;
use std::path::PathBuf;
use std::process::Stdio;
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::Command;

static PROGRESS_RE: OnceCell<Regex> = OnceCell::new();

fn progress_re() -> &'static Regex {
    PROGRESS_RE.get_or_init(|| Regex::new(r"(?i)progress:?\s*(\d+(?:\.\d+)?)\s*%").unwrap())
}

pub fn parse_percent(line: &str) -> Option<f64> {
    progress_re()
        .captures(line)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

pub async fn run_job(
    torrent_url: &str,
    download_dir: &str,
    size_bytes: u64,
    on_progress: impl Fn(u64) + Send + 'static,
) -> Result<()> {
    let dir = PathBuf::from(download_dir);
    tokio::fs::create_dir_all(&dir)
        .await
        .with_context(|| format!("creating {}", dir.display()))?;

    let mut cmd = Command::new("transmission-cli");
    cmd.arg(torrent_url)
        .arg("-w")
        .arg(&dir)
        .arg("--no-prompt")
        .arg("--no-trash-torrent")
        .kill_on_drop(true)
        .stdout(Stdio::null())
        .stderr(Stdio::piped());

    let mut child = cmd.spawn().context("spawning transmission-cli")?;
    let stderr = child.stderr.take().context("piping stderr")?;
    let mut lines = BufReader::new(stderr).lines();

    let size_f = size_bytes as f64;
    let reader = tokio::spawn(async move {
        while let Ok(Some(line)) = lines.next_line().await {
            if let Some(pct) = parse_percent(&line) {
                let bytes = if size_f > 0.0 {
                    (pct / 100.0 * size_f) as u64
                } else {
                    0
                };
                on_progress(bytes);
            }
        }
    });

    let status = child.wait().await.context("waiting on transmission-cli")?;
    let _ = reader.await;
    status
        .success()
        .then_some(())
        .context("transmission-cli exited non-zero")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_capital_progress() {
        assert_eq!(
            parse_percent("Progress: 45.32%, dl from 50 of 200 peers"),
            Some(45.32)
        );
    }

    #[test]
    fn parses_lowercase_progress() {
        assert_eq!(
            parse_percent("[Connected] progress: 12.0%, dl from 3 of 50 peers"),
            Some(12.0)
        );
    }

    #[test]
    fn parses_no_space() {
        assert_eq!(parse_percent("Progress:99%"), Some(99.0));
    }

    #[test]
    fn ignores_unrelated_lines() {
        assert_eq!(parse_percent("Connecting to peers..."), None);
        assert_eq!(parse_percent("Transmission 3.00"), None);
    }
}
