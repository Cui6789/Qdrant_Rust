//! Reusable Axum API and application router for the Qdrant_Rust dashboard.
//!
//! The binary (`src/main.rs`) is only a launcher; this library owns the HTTP
//! surface so another Rust service can mount it directly.
//!
//! # Example
//!
//! ```ignore
//! use kb_core::config::AppConfig;
//! use kb_dashboard::{build_app, resolve_ui_dir, AppState};
//!
//! let config = AppConfig::load(None)?;
//! let state = AppState::from_config(&config)?;
//! let app = build_app(state, resolve_ui_dir());
//! // axum::serve(listener, app).await?;
//! ```
//!
//! Use [`build_api`] to mount only `/api`, and [`build_app`] to serve the
//! bundled Yew frontend as well.

pub mod dashscope;
pub mod error;
pub mod handlers;
pub mod pipeline;
pub mod state;

use std::path::{Path, PathBuf};
use std::time::Duration;

use axum::extract::DefaultBodyLimit;
use axum::http::StatusCode;
use axum::middleware;
use axum::response::{Html, IntoResponse, Response};
use axum::routing::{delete, get, post};
use axum::{Json, Router};
use serde_json::json;
use tower_http::cors::{Any, CorsLayer};
use tower_http::services::{ServeDir, ServeFile};
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

pub use error::ApiError;
pub use state::{AppState, EmbeddingRuntime, JobStore};

/// Default port of the dashboard (kept separate from kb-server's 8080).
pub const DEFAULT_PORT: u16 = 8090;
/// Default bind address: loopback only, because the API can hold an API key.
pub const DEFAULT_HOST: &str = "127.0.0.1";
/// Maximum JSON request body accepted by the dashboard.
pub const MAX_BODY_BYTES: usize = 16 * 1024 * 1024;
/// Upper bound for a synchronous dashboard request (search/embedding probe).
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(120);

/// Builds the `/api` router, including body limits, timeouts, tracing and CORS.
pub fn build_api(state: AppState) -> Router {
    Router::new()
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
        .with_state(state)
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

/// Builds the complete application: `/api` plus the built Yew frontend.
///
/// When `ui_dir` does not contain `index.html`, the server still exposes the
/// API and serves a small page explaining how to build the frontend.
pub fn build_app(state: AppState, ui_dir: impl AsRef<Path>) -> Router {
    let ui_dir = ui_dir.as_ref();
    let index = ui_dir.join("index.html");
    let api = build_api(state);
    if index.exists() {
        let static_files = ServeDir::new(ui_dir).fallback(ServeFile::new(&index));
        Router::new()
            .nest("/api", api)
            .fallback_service(static_files)
    } else {
        Router::new()
            .nest("/api", api)
            .route("/", get(missing_ui_page))
            .fallback(missing_ui_page)
    }
}

/// Finds the built frontend bundle.
///
/// Order: `DASHBOARD_UI_DIR`, the current directory layout, the crate-relative
/// layout (when launched from `crates/kb-dashboard`), and finally the absolute
/// location recorded at compile time.
pub fn resolve_ui_dir() -> PathBuf {
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
