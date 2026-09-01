use anyhow::{Context, Result};
use once_cell::sync::OnceCell;
use regex::Regex;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::{hash_map::Entry, HashMap};
use std::time::Duration;

#[derive(Debug, Clone, Serialize)]
pub struct SearchResult {
    pub display_title: String,
    pub year: u32,
    pub quality: String,
    pub seeds: u32,
    pub leechers: u32,
    pub size_bytes: u64,
    pub magnet: String,
}

#[derive(Debug, Deserialize)]
struct RawItem {
    name: String,
    #[serde(default)]
    info_hash: String,
    #[serde(default, deserialize_with = "deserialize_u32")]
    seeders: u32,
    #[serde(default, deserialize_with = "deserialize_u32")]
    leechers: u32,
    #[serde(default, deserialize_with = "deserialize_u64")]
    size: u64,
    #[serde(default, deserialize_with = "deserialize_u32")]
    category: u32,
}

static QUALITY_RE: OnceCell<Regex> = OnceCell::new();
static CLIENT: OnceCell<reqwest::Client> = OnceCell::new();
static TRACKERS: &[&str] = &[
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://tracker.openbittorrent.com:6969/announce",
    "udp://9.rarbg.to:2710/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://open.stealth.si:80/announce",
];

fn quality_re() -> &'static Regex {
    QUALITY_RE.get_or_init(|| Regex::new(r"(?i)\b(\d{3,4}p|4k)\b").unwrap())
}

fn client() -> &'static reqwest::Client {
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent("Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/126.0.0.0 Safari/537.36")
            .connect_timeout(Duration::from_secs(5))
            .timeout(Duration::from_secs(15))
            .build()
            .expect("building HTTP client")
    })
}

async fn fetch_json<T: for<'de> Deserialize<'de>>(url: &str) -> Result<Vec<T>> {
    let resp = client()
        .get(url)
        .send()
        .await
        .with_context(|| format!("GET {url}"))?
        .error_for_status()
        .with_context(|| format!("HTTP {url}"))?;
    let items: Vec<T> = resp.json().await.with_context(|| format!("JSON {url}"))?;
    Ok(items)
}

pub async fn search(query: &str, resolution: &str) -> Result<Vec<SearchResult>> {
    let mut by_hash: HashMap<String, RawItem> = HashMap::new();
    let query = query.trim();

    if query.is_empty() {
        return Ok(Vec::new());
    }

    tracing::info!("search: q={query:?}");

    let url = format!("https://apibay.org/q.php?q={}", urlencode(query));
    let primary = fetch_json::<RawItem>(&url).await;
    let primary_failed = primary.is_err();
    match primary {
        Ok(items) => insert_items(&mut by_hash, items, None),
        Err(e) => tracing::warn!("apibay search failed: {e:#}"),
    }

    if by_hash.is_empty() {
        let q_lower = query.to_lowercase();
        let (movies, hd_movies, tv, hd_tv) = tokio::join!(
            fetch_json::<RawItem>("https://apibay.org/precompiled/data_top100_201.json"),
            fetch_json::<RawItem>("https://apibay.org/precompiled/data_top100_207.json"),
            fetch_json::<RawItem>("https://apibay.org/precompiled/data_top100_208.json"),
            fetch_json::<RawItem>("https://apibay.org/precompiled/data_top100_209.json"),
        );

        let mut fallback_succeeded = false;
        for (cat, result) in [(201, movies), (207, hd_movies), (208, tv), (209, hd_tv)] {
            match result {
                Ok(items) => {
                    fallback_succeeded = true;
                    insert_items(&mut by_hash, items, Some(&q_lower));
                }
                Err(e) => tracing::warn!("precompiled cat={cat} failed: {e:#}"),
            }
        }

        if primary_failed && !fallback_succeeded {
            anyhow::bail!("all search sources failed");
        }
    }

    let mut results: Vec<SearchResult> = by_hash
        .into_values()
        .map(build_result)
        .filter(|result| matches_resolution(&result.quality, resolution))
        .collect();
    results.sort_by_key(|result| std::cmp::Reverse(result.seeds));
    results.truncate(20);
    Ok(results)
}

fn matches_resolution(quality: &str, resolution: &str) -> bool {
    match resolution.to_ascii_lowercase().as_str() {
        "" | "any" => true,
        "2160p" => quality.eq_ignore_ascii_case("2160p") || quality.eq_ignore_ascii_case("4k"),
        selected => quality.eq_ignore_ascii_case(selected),
    }
}

fn insert_items(
    by_hash: &mut HashMap<String, RawItem>,
    items: Vec<RawItem>,
    query_filter: Option<&str>,
) {
    for item in items {
        if !(200..300).contains(&item.category) || !valid_hash(&item.info_hash) {
            continue;
        }
        if query_filter.is_some_and(|query| !item.name.to_lowercase().contains(query)) {
            continue;
        }

        let hash = item.info_hash.to_uppercase();
        match by_hash.entry(hash) {
            Entry::Vacant(entry) => {
                entry.insert(item);
            }
            Entry::Occupied(mut entry) if item.seeders > entry.get().seeders => {
                entry.insert(item);
            }
            Entry::Occupied(_) => {}
        }
    }
}

fn valid_hash(hash: &str) -> bool {
    hash.len() == 40
        && hash.bytes().all(|b| b.is_ascii_hexdigit())
        && hash.bytes().any(|b| b != b'0')
}

fn deserialize_u32<'de, D>(deserializer: D) -> Result<u32, D::Error>
where
    D: Deserializer<'de>,
{
    deserialize_u64(deserializer).and_then(|value| {
        u32::try_from(value).map_err(|_| D::Error::custom("integer does not fit in u32"))
    })
}

fn deserialize_u64<'de, D>(deserializer: D) -> Result<u64, D::Error>
where
    D: Deserializer<'de>,
{
    match Value::deserialize(deserializer)? {
        Value::Number(value) => value
            .as_u64()
            .ok_or_else(|| D::Error::custom("invalid integer")),
        Value::String(value) => value.parse().map_err(D::Error::custom),
        Value::Null => Ok(0),
        _ => Err(D::Error::custom("expected integer or numeric string")),
    }
}

fn build_result(r: RawItem) -> SearchResult {
    let quality = quality_re()
        .find(&r.name)
        .map(|m| m.as_str().to_string())
        .unwrap_or_else(|| "?".to_string());

    let parsed = crate::naming::parse(&r.name);
    let year = parsed.year.unwrap_or(0);
    let display_title = if parsed.title.is_empty() {
        r.name.clone()
    } else {
        parsed.title
    };

    let dn = urlencode(&display_title);
    let tr = TRACKERS
        .iter()
        .map(|t| format!("&tr={}", urlencode(t)))
        .collect::<String>();
    let magnet = format!(
        "magnet:?xt=urn:btih:{}&dn={}{}",
        r.info_hash.to_uppercase(),
        dn,
        tr
    );

    SearchResult {
        display_title,
        year,
        quality,
        seeds: r.seeders,
        leechers: r.leechers,
        size_bytes: r.size,
        magnet,
    }
}

fn urlencode(s: &str) -> String {
    s.bytes()
        .flat_map(|b| {
            if b.is_ascii_alphanumeric() || b"-_.~".contains(&b) {
                vec![b as char]
            } else {
                format!("%{:02X}", b).chars().collect()
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_api_string_fields() {
        let items: Vec<RawItem> = serde_json::from_str(
            r#"[{"name":"Aliens 1986 1080p","info_hash":"717F7E9D1DA87217591C4563274381E56D91B132","seeders":"437","leechers":"41","size":"2831246493","category":"207"}]"#,
        )
        .unwrap();

        assert_eq!(items[0].seeders, 437);
        assert_eq!(items[0].leechers, 41);
        assert_eq!(items[0].size, 2_831_246_493);
        assert_eq!(items[0].category, 207);
    }

    #[test]
    fn parses_precompiled_numeric_fields() {
        let items: Vec<RawItem> = serde_json::from_str(
            r#"[{"name":"Aliens 1986 1080p","info_hash":"717F7E9D1DA87217591C4563274381E56D91B132","seeders":437,"leechers":41,"size":2831246493,"category":207}]"#,
        )
        .unwrap();

        assert_eq!(items[0].seeders, 437);
        assert_eq!(items[0].category, 207);
    }

    #[test]
    fn rejects_sentinel_and_non_video_results() {
        let items = vec![
            RawItem {
                name: "No results returned".into(),
                info_hash: "0000000000000000000000000000000000000000".into(),
                seeders: 0,
                leechers: 0,
                size: 0,
                category: 0,
            },
            RawItem {
                name: "Aliens soundtrack".into(),
                info_hash: "717F7E9D1DA87217591C4563274381E56D91B132".into(),
                seeders: 50,
                leechers: 1,
                size: 100,
                category: 101,
            },
        ];
        let mut by_hash = HashMap::new();

        insert_items(&mut by_hash, items, None);

        assert!(by_hash.is_empty());
    }

    #[test]
    fn filters_resolution_and_treats_4k_as_2160p() {
        assert!(matches_resolution("1080p", "any"));
        assert!(matches_resolution("1080p", "1080p"));
        assert!(!matches_resolution("720p", "1080p"));
        assert!(matches_resolution("4K", "2160p"));
    }
}
