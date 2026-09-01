use regex::Regex;
use std::sync::OnceLock;

#[derive(Debug, Clone, PartialEq)]
pub enum Kind {
    Movie,
    Episode,
}

#[derive(Debug, Clone)]
pub struct Parsed {
    pub kind: Kind,
    pub title: String,
    pub year: Option<u32>,
    pub season: Option<u32>,
    pub episode: Option<u32>,
}

impl Parsed {
    pub fn final_name(&self, ext: &str, quality: Option<&str>) -> String {
        let q = quality.map(|q| format!(" ({q})")).unwrap_or_default();
        match self.kind {
            Kind::Movie => match self.year {
                Some(y) => format!("{} - {}{}.{}", self.title, y, q, ext),
                None => format!("{}{}.{}", self.title, q, ext),
            },
            Kind::Episode => {
                let s = self.season.unwrap_or(0);
                let e = self.episode.unwrap_or(0);
                format!("{} - S{:02}E{:02}{}.{}", self.title, s, e, q, ext)
            }
        }
    }
}

static EP_RE: OnceLock<Regex> = OnceLock::new();
static YEAR_RE: OnceLock<Regex> = OnceLock::new();

fn ep_re() -> &'static Regex {
    EP_RE.get_or_init(|| Regex::new(r"(?i)S(\d{1,2})E(\d{1,2})").unwrap())
}

fn year_re() -> &'static Regex {
    YEAR_RE.get_or_init(|| Regex::new(r"(?:\(|\b)(19\d{2}|20\d{2})(?:\)|\b)").unwrap())
}

pub fn parse(raw: &str) -> Parsed {
    let cleaned = raw.trim();

    if let Some(caps) = ep_re().captures(cleaned) {
        let season: u32 = caps[1].parse().unwrap_or(0);
        let episode: u32 = caps[2].parse().unwrap_or(0);
        let title = title_before(cleaned, caps.get(0).unwrap().start());
        return Parsed {
            kind: Kind::Episode,
            title,
            year: None,
            season: Some(season),
            episode: Some(episode),
        };
    }

    let year = year_re().find(cleaned).and_then(|m| {
        m.as_str()
            .chars()
            .filter(|c| c.is_ascii_digit())
            .collect::<String>()
            .parse()
            .ok()
    });

    let title_end = year_re()
        .find(cleaned)
        .map(|m| m.start())
        .unwrap_or(cleaned.len());

    Parsed {
        kind: Kind::Movie,
        title: title_before(cleaned, title_end),
        year,
        season: None,
        episode: None,
    }
}

fn title_before(raw: &str, end: usize) -> String {
    let head = &raw[..end];
    let stripped = head.replace(['.', '_'], " ").replace(['[', ']'], "");
    let trimmed = stripped.trim_end_matches(['-', ' ', '(']).trim();
    collapse_ws(trimmed).to_string()
}

fn collapse_ws(s: &str) -> &str {
    let mut start = 0;
    let mut end = s.len();
    let bytes = s.as_bytes();
    while start < end && bytes[start].is_ascii_whitespace() {
        start += 1;
    }
    while end > start && bytes[end - 1].is_ascii_whitespace() {
        end -= 1;
    }
    &s[start..end]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn movie_with_dup_year() {
        let p = parse("Interstellar (2014) (2014) 1080p BrRip x264 - YIFY");
        assert_eq!(p.kind, Kind::Movie);
        assert_eq!(p.title, "Interstellar");
        assert_eq!(p.year, Some(2014));
        assert_eq!(p.final_name("mkv", None), "Interstellar - 2014.mkv");
        assert_eq!(
            p.final_name("mkv", Some("1080p")),
            "Interstellar - 2014 (1080p).mkv"
        );
    }

    #[test]
    fn movie_dotted() {
        let p = parse("The.Matrix.1999.1080p.BluRay.x264.YIFY");
        assert_eq!(p.kind, Kind::Movie);
        assert_eq!(p.title, "The Matrix");
        assert_eq!(p.year, Some(1999));
    }

    #[test]
    fn episode_dashed() {
        let p = parse("Seinfeld - S01E02 - The Stakeout 1080p");
        assert_eq!(p.kind, Kind::Episode);
        assert_eq!(p.title, "Seinfeld");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(2));
        assert_eq!(p.final_name("mkv", None), "Seinfeld - S01E02.mkv");
    }

    #[test]
    fn episode_dotted() {
        let p = parse("Breaking.Bad.S01E01.720p.HDTV.x264-COOL");
        assert_eq!(p.kind, Kind::Episode);
        assert_eq!(p.title, "Breaking Bad");
        assert_eq!(p.season, Some(1));
        assert_eq!(p.episode, Some(1));
        assert_eq!(p.final_name("mkv", None), "Breaking Bad - S01E01.mkv");
    }
}
