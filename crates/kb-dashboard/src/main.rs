//! Launcher for the Qdrant_Rust visual dashboard.
//!
//! The HTTP surface and reusable state live in the library crate
//! (`src/lib.rs`); this file only parses startup arguments, probes Qdrant,
//! builds the router and serves it.

use std::path::PathBuf;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use kb_core::config::AppConfig;
use kb_dashboard::{build_app, resolve_ui_dir, AppState, DEFAULT_HOST, DEFAULT_PORT};
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

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
    let state = AppState::from_config(&config)?;
    let store = state.store.clone();

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

    let ui_dir = resolve_ui_dir();
    let index = ui_dir.join("index.html");
    if !index.exists() {
        tracing::warn!(
            ui_dir = %ui_dir.display(),
            "frontend bundle not found; run `trunk build --release` in crates/kb-dashboard-ui"
        );
    }

    let app = build_app(state.clone(), &ui_dir);

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
