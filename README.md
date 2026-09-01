# autotorrent

LAN-only torrent search + downloader. Rust (axum + Tera), apibay.org, transmission-cli.

## Stack

- Backend: axum + Tera templates
- Search: apibay.org query API, with popular movie/TV feeds as fallback
- Downloader: `transmission-cli` (spawned per job)
- Single binary, no DB, no config UI

## Setup

1. `brew install transmission-cli` (one-time)
2. `cargo run`
3. Open `http://<your-laptop-ip>:8080`

To find your local IP: `ipconfig getifaddr en0` (Mac, Wi-Fi).

## Config (env vars)

| Var | Default | Purpose |
|---|---|---|
| `PORT` | `8080` | HTTP port |
| `DOWNLOAD_DIR` | `./downloads` | Where files land |
| `RUST_LOG` | `info` | Tracing level |

`.env.example` ships with the defaults.

## How naming works

- `Interstellar (2014) (2014) 1080p BrRip x264 - YIFY` → `Interstellar - 2014 (1080p).mkv`
- Top 20 video results sorted by seeders.
- Optional 720p, 1080p, or 2160p/4K filter.
- Quality parsed from torrent name (1080p, 2160p, 720p, 4K).
- Always `.mkv` extension.

## Phone usage

Same URL over Wi-Fi. Add to Home Screen for app-like behavior.

## Caveats

- No auth. LAN-only by design.
- Server crash kills in-flight downloads (`kill_on_drop`). Restart = re-search.
- Process restart loses download history (in-memory only).
- apibay query results are filtered to video categories; popular movie/TV feeds fill gaps when query search returns nothing.
