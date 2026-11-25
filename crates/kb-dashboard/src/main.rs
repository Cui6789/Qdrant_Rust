//! Visual dashboard for the Qdrant_Rust knowledge base.
//!
//! Runs a small Axum API, reuses the `kb-core` pipeline, and serves the Yew
//! WebAssembly frontend from `crates/kb-dashboard-ui/dist`.

mod dashscope;
mod error;
mod handlers;
mod pipeline;
mod state;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use anyhow::{bail, Context, Result};
use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use chrono::Utc;
use kb_core::config::AppConfig;
use kb_core::store::QdrantStore;
use kb_dashboard_types::EmbeddingDefaults;
use serde_json::json;
use state::{AppState, EmbeddingRuntime, JobStore};
use tokio::net::TcpListener;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;
use tracing_subscriber::EnvFilter;

/// Default port of the dashboard (kept separate from kb-server's 8080).
const DEFAULT_PORT: u16 = 8090;
/// Default bind address: loopback only, because the API can hold an API key.
const DEFAULT_HOST: &str = "127.0.0.1";
/// Maximum JSON request body accepted by the dashboard.
const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound for a synchronous dashboard request (search/embedding probe).
const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Command-line arguments.
struct Args {
    config: Option<PathBuf>,
    host: String,
    port: u16,
}

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();
    let args = parse_args()?;
    let config = AppConfig::load(args.config.as_deref()).context("加载配置失败")?;

    let store = Arc::new(QdrantStore::new(&config.qdrant).context("创建 Qdrant 客户端失败")?);

    // The dashboard must stay usable while Qdrant is down: probe once, log,
    // and let the UI surface the degraded state instead of exiting.
    match tokio::time::timeout(Duration::from_secs(2), store.list_collections()).await {
        Ok(Ok(collections)) => {
            tracing::info!(
                qdrant_url = %config.qdrant.url,
                collections = collections.len(),
                "Qdrant reachable"
            );
        }
        Ok(Err(error)) => {
            tracing::warn!(
                qdrant_url = %config.qdrant.url,
                error = %error,
                "Qdrant is not reachable yet; the dashboard will show a degraded state"
            );
        }
        Err(_) => {
            tracing::warn!(
                qdrant_url = %config.qdrant.url,
                "Qdrant probe timed out; the dashboard will show a degraded state"
            );
        }
    }

    let state = AppState {
        store,
        qdrant_url: config.qdrant.url.clone(),
        qdrant_api_key_configured: !config.qdrant.api_key.trim().is_empty(),
        ingest: config.ingest.clone(),
        embedding: Arc::new(tokio::sync::RwLock::new(EmbeddingRuntime::default())),
        defaults: EmbeddingDefaults {
            base_url: config.embedding.api.base_url.clone(),
            model: config.embedding.api.model.clone(),
            dimensions: config.embedding.api.dimensions,
        },
        embedding_timeout_secs: config.embedding.api.timeout_secs,
        embedding_max_retries: config.embedding.api.max_retries,
        embedding_batch_size: config.embedding.batch_size,
        embedding_max_concurrent_batches: config.embedding.max_concurrent_batches,
        jobs: Arc::new(Mutex::new(JobStore::default())),
        started_instant: Instant::now(),
        started_at: Utc::now(),
        version: env!("CARGO_PKG_VERSION").to_string(),
    };

    let ui_dir = resolve_ui_dir();
    let index = ui_dir.join("index.html");
    if !index.exists() {
        tracing::warn!(
            ui_dir = %ui_dir.display(),
            "frontend bundle not found; run `trunk build --release` in crates/kb-dashboard-ui"
        );
    }

    let api = build_api(state.clone());
    let app = if index.exists() {
        let static_files = ServeDir::new(&ui_dir).fallback(ServeFile::new(&index));
        Router::new()
            .nest("/api", api)
            .fallback_service(static_files)
    } else {
        Router::new()
            .nest("/api", api)
            .route("/", get(missing_ui_page))
            .fallback(missing_ui_page)
    };

    let address = format!("{}:{}", args.host, args.port);
    let listener = TcpListener::bind(&address)
        .await
        .with_context(|| format!("无法监听 {address}"))?;
    tracing::info!(
        address = %address,
        ui_dir = %ui_dir.display(),
        qdrant_url = %config.qdrant.url,
        qdrant_auth = state.qdrant_api_key_configured,
        "kb-dashboard listening"
    );
    if args.host != "127.0.0.1" && args.host != "localhost" && args.host != "::1" {
        tracing::warn!(
            host = %args.host,
            "the dashboard holds an embedding API key in memory and has no authentication; bind it to loopback unless you put it behind an authenticated proxy"
        );
    }

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;
    tracing::info!("kb-dashboard stopped");
    Ok(())
}

/// Builds the `/api` router.
fn build_api(state: AppState) -> Router {
    let router = Router::new()
        .route("/health", get(handlers::health))
        .route("/overview", get(handlers::overview))
        .route(
            "/collections",
            get(handlers::list_collections).post(handlers::create_collection),
        )
        .route("/collections/{name}", delete(handlers::delete_collection))
        .route(
            "/collections/{name}/documents",
            get(handlers::list_documents),
        )
        .route(
            "/collections/{name}/document",
            get(handlers::document_detail).delete(handlers::delete_document),
        )
        .route("/collections/{name}/search", post(handlers::search))
        .route(
            "/embedding/config",
            get(handlers::get_embedding_config)
                .post(handlers::set_embedding_config)
                .delete(handlers::clear_embedding_config),
        )
        .route("/embedding/mock", post(handlers::set_mock_embedding))
        .route("/pipeline/run", post(handlers::run_pipeline))
        .route("/pipeline/jobs/{id}", get(handlers::get_pipeline_job))
        .with_state(state);

    router
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            REQUEST_TIMEOUT,
        ))
        .layer(middleware::map_response(rewrite_status_envelopes))
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
        .layer(
            CorsLayer::new()
                .allow_origin(Any)
                .allow_methods(Any)
                .allow_headers(Any),
        )
}

/// Rewrites empty middleware responses (408/413) into the shared envelope.
async fn rewrite_status_envelopes(response: Response) -> Response {
    let (code, message) = match response.status() {
        StatusCode::REQUEST_TIMEOUT => ("REQUEST_TIMEOUT", "请求超时"),
        StatusCode::PAYLOAD_TOO_LARGE => ("PAYLOAD_TOO_LARGE", "请求体过大"),
        _ => return response,
    };
    (
        response.status(),
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}

/// Landing page shown when the frontend bundle has not been built yet.
async fn missing_ui_page() -> Html<&'static str> {
    Html(
        r#"<!doctype html>
<html lang="zh-CN">
<head><meta charset="utf-8"><title>Qdrant_Rust Studio</title>
<style>
body{margin:0;min-height:100vh;display:grid;place-items:center;background:#f5f5f7;color:#1d1d1f;font-family:-apple-system,BlinkMacSystemFont,"Segoe UI","Microsoft YaHei",sans-serif}
main{max-width:640px;padding:48px;background:#fff;border-radius:28px;box-shadow:0 20px 60px rgba(0,0,0,.08)}
h1{font-size:32px;letter-spacing:-.02em;margin:0 0 12px}
p{color:#6e6e73;line-height:1.6}
code{background:#f5f5f7;padding:4px 8px;border-radius:8px;font-size:14px}
</style></head>
<body><main>
<h1>前端尚未构建</h1>
<p>Rust API 已在运行，但没有找到 <code>crates/kb-dashboard-ui/dist/index.html</code>。</p>
<p>在仓库根目录执行：</p>
<p><code>cd crates/kb-dashboard-ui &amp;&amp; trunk build --release</code></p>
<p>然后刷新此页面。</p>
</main></body></html>"#,
    )
}

/// Finds the built frontend bundle.
///
/// Order: `DASHBOARD_UI_DIR`, the current directory layout, the crate-relative
/// layout (when launched from `crates/kb-dashboard`), and finally the absolute
/// location recorded at compile time.
fn resolve_ui_dir() -> PathBuf {
    if let Ok(raw) = std::env::var("DASHBOARD_UI_DIR") {
        let trimmed = raw.trim();
        if !trimmed.is_empty() {
            return PathBuf::from(trimmed);
        }
    }
    let candidates = [
        PathBuf::from("crates/kb-dashboard-ui/dist"),
        PathBuf::from("../kb-dashboard-ui/dist"),
        PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../kb-dashboard-ui/dist"),
    ];
    for candidate in &candidates {
        if candidate.join("index.html").exists() {
            return candidate.clone();
        }
    }
    candidates[0].clone()
}

/// Parses `--config`, `--host`, and `--port` (also `--key=value` forms).
fn parse_args() -> Result<Args> {
    let mut config = std::env::var("KB_CONFIG")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .map(PathBuf::from);
    let mut host = std::env::var("DASHBOARD_HOST")
        .ok()
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_HOST.to_string());
    let mut port = std::env::var("DASHBOARD_PORT")
        .ok()
        .and_then(|value| value.trim().parse::<u16>().ok())
        .unwrap_or(DEFAULT_PORT);

    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        let (key, inline_value) = match argument.split_once('=') {
            Some((key, value)) => (key.to_string(), Some(value.to_string())),
            None => (argument.clone(), None),
        };
        let mut take_value = |name: &str| -> Result<String> {
            inline_value
                .clone()
                .or_else(|| args.next())
                .with_context(|| format!("{name} 需要一个参数"))
        };
        match key.as_str() {
            "--config" => config = Some(PathBuf::from(take_value("--config")?)),
            "--host" => host = take_value("--host")?,
            "--port" => {
                let value = take_value("--port")?;
                port = value
                    .parse::<u16>()
                    .with_context(|| format!("无效端口: {value}"))?;
            }
            "--help" | "-h" => {
                println!(
                    "kb-dashboard [--config <path>] [--host <addr>] [--port <port>]\n\
                     \n\
                     Environment: DASHBOARD_HOST, DASHBOARD_PORT, DASHBOARD_UI_DIR,\n\
                     KB_CONFIG, KB_QDRANT__URL (gRPC 6334)."
                );
                std::process::exit(0);
            }
            other => bail!("未知参数: {other}（使用 --help 查看用法）"),
        }
    }
    Ok(Args { config, host, port })
}

fn init_tracing() {
    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("info,kb_dashboard=debug,kb_core=info"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

async fn shutdown_signal() {
    let ctrl_c = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to listen for Ctrl-C");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to listen for SIGTERM"),
        }
    };

    #[cfg(not(unix))]
    let terminate = std::future::pending::<()>();

    tokio::select! {
        _ = ctrl_c => {},
        _ = terminate => {},
    }
    tracing::info!("shutdown signal received");
}
