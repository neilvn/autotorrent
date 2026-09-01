mod download;
mod naming;
mod search;

use anyhow::Result;
use axum::{
    extract::{Form, State},
    response::{Html, IntoResponse, Redirect, Response},
    routing::{get, post},
    Router,
};
use once_cell::sync::Lazy;
use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    net::SocketAddr,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};
use tera::{Context, Tera};
use tokio::sync::Mutex;
use tower_http::services::ServeDir;

static TERA: Lazy<Tera> = Lazy::new(|| {
    let mut t = Tera::new("templates/**/*").expect("loading templates");
    t.autoescape_on(vec!["html", "htm"]);
    t
});

#[derive(Clone, Serialize)]
struct Download {
    id: u64,
    query: String,
    title: String,
    final_name: String,
    download_dir: String,
    status: String,
    bytes_downloaded: u64,
    size_bytes: u64,
    quality: String,
}

#[derive(Clone)]
struct AppState {
    downloads: Arc<Mutex<HashMap<u64, Download>>>,
    next_id: Arc<AtomicU64>,
}

#[derive(Deserialize)]
struct SearchForm {
    q: String,
    resolution: String,
}

#[derive(Deserialize)]
struct DownloadForm {
    title: String,
    year: u32,
    quality: String,
    size_bytes: u64,
    magnet: String,
}

#[derive(Deserialize)]
struct MarkComplete {
    id: u64,
}

#[derive(Clone)]
struct Config {
    download_dir: String,
    port: u16,
}

fn load_config() -> Config {
    let download_dir = std::env::var("DOWNLOAD_DIR").unwrap_or_else(|_| "./downloads".into());
    let port = std::env::var("PORT")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(8080);
    Config { download_dir, port }
}

async fn index(State(state): State<AppState>) -> Response {
    let downloads: Vec<Download> = state.downloads.lock().await.values().cloned().collect();
    let mut ctx = Context::new();
    ctx.insert("downloads", &downloads);
    render("index.html", &ctx).into_response()
}

async fn search_route(State(state): State<AppState>, Form(form): Form<SearchForm>) -> Response {
    match search::search(&form.q, &form.resolution).await {
        Ok(results) => {
            let downloads: Vec<Download> = state.downloads.lock().await.values().cloned().collect();
            let mut ctx = Context::new();
            ctx.insert("query", &form.q);
            ctx.insert("resolution", &form.resolution);
            ctx.insert("results", &results);
            ctx.insert("downloads", &downloads);
            render("results.html", &ctx).into_response()
        }
        Err(e) => {
            let mut ctx = Context::new();
            ctx.insert("query", &form.q);
            ctx.insert("resolution", &form.resolution);
            ctx.insert("error", &format!("Search failed: {e}"));
            ctx.insert("results", &Vec::<search::SearchResult>::new());
            ctx.insert("downloads", &Vec::<Download>::new());
            render("results.html", &ctx).into_response()
        }
    }
}

async fn download_route(
    State(state): State<AppState>,
    config: axum::extract::Extension<Config>,
    Form(form): Form<DownloadForm>,
) -> Response {
    let id = state.next_id.fetch_add(1, Ordering::SeqCst);

    let parsed = if form.year > 0 {
        naming::parse(&format!("{} ({})", form.title, form.year))
    } else {
        naming::parse(&form.title)
    };
    let final_name = parsed.final_name("mkv", Some(&form.quality));

    let entry = Download {
        id,
        query: String::new(),
        title: form.title.clone(),
        final_name: final_name.clone(),
        download_dir: config.download_dir.clone(),
        status: "running".into(),
        bytes_downloaded: 0,
        size_bytes: form.size_bytes,
        quality: form.quality.clone(),
    };
    {
        let mut d = state.downloads.lock().await;
        d.insert(id, entry);
    }

    let downloads = state.downloads.clone();
    let title = form.title.clone();
    let magnet = form.magnet.clone();
    let dir = config.download_dir.clone();
    let quality = form.quality.clone();
    let size_bytes = form.size_bytes;

    let downloads_for_cb = downloads.clone();
    let downloads_for_final = downloads.clone();

    tokio::spawn(async move {
        let res = download::run_job(&magnet, &dir, size_bytes, move |bytes| {
            let downloads = downloads_for_cb.clone();
            tokio::spawn(async move {
                let mut d = downloads.lock().await;
                if let Some(entry) = d.get_mut(&id) {
                    entry.bytes_downloaded = bytes;
                }
            });
        })
        .await;

        let final_status = if res.is_ok() { "complete" } else { "failed" };
        let mut d = downloads_for_final.lock().await;
        if let Some(entry) = d.get_mut(&id) {
            entry.status = final_status.into();
        }
        if let Err(e) = res {
            tracing::warn!("download {id} ({title} {quality}) failed: {e}");
        }
    });

    Redirect::to("/").into_response()
}

async fn complete_route(State(state): State<AppState>, Form(form): Form<MarkComplete>) -> Response {
    {
        let mut d = state.downloads.lock().await;
        if let Some(entry) = d.get_mut(&form.id) {
            if entry.status == "running" {
                entry.status = "complete".into();
            }
        }
    }
    Redirect::to("/").into_response()
}

fn render(name: &str, ctx: &Context) -> Html<String> {
    match TERA.render(name, ctx) {
        Ok(s) => Html(s),
        Err(e) => Html(format!("<pre>template error in {name}: {e:#?}</pre>")),
    }
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();

    let config = load_config();
    tokio::fs::create_dir_all(&config.download_dir).await.ok();

    let state = AppState {
        downloads: Arc::new(Mutex::new(HashMap::new())),
        next_id: Arc::new(AtomicU64::new(1)),
    };

    let app = Router::new()
        .route("/", get(index))
        .route("/search", post(search_route))
        .route("/download", post(download_route))
        .route("/complete", post(complete_route))
        .nest_service("/static", ServeDir::new("static"))
        .layer(axum::extract::Extension(config.clone()))
        .with_state(state);

    let local_ip = local_ip().unwrap_or_else(|_| "0.0.0.0".parse().unwrap());
    let addr = SocketAddr::from(([0, 0, 0, 0], config.port));
    tracing::info!("listening on http://{local_ip}:{}", config.port);
    tracing::info!("downloads → {}", config.download_dir);

    let listener = tokio::net::TcpListener::bind(addr).await?;
    axum::serve(listener, app).await?;
    Ok(())
}

fn local_ip() -> std::io::Result<std::net::IpAddr> {
    let socket = std::net::UdpSocket::bind("0.0.0.0:0")?;
    socket.connect("8.8.8.8:80")?;
    Ok(socket.local_addr()?.ip())
}
