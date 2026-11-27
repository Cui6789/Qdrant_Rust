//! Knowledge base HTTP server entry point.

use std::env;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use axum::Router;
use kb_core::config::AppConfig;
use kb_core::models::EmbeddingInfo;
use kb_core::store::QdrantStore;
use kb_server::api::build_managed_router;
use kb_server::management::Management;
use kb_server::state::AppState;
use tokio::net::TcpListener;
use tokio::sync::watch;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    init_tracing();

    let config_path = parse_config_path()?;
    let config = AppConfig::load(config_path.as_deref()).context("failed to load configuration")?;

    // Startup logs never include API keys.
    tracing::info!(
        host = %config.server.host,
        port = config.server.port,
        qdrant_url = %config.qdrant.url,
        auth_enabled = !config.server.api_key.is_empty(),
        max_body_mb = config.server.max_body_mb,
        request_timeout_secs = config.server.request_timeout_secs,
        "starting kb-server"
    );

    let store =
        Arc::new(QdrantStore::new(&config.qdrant).context("failed to build the Qdrant client")?);
    store
        .wait_ready(Duration::from_secs(config.qdrant.startup_retry_secs))
        .await
        .context("Qdrant is not ready")?;

    let management = Arc::new(
        Management::new(config.clone(), store.clone())
            .await
            .context("failed to initialize knowledge base providers")?,
    );
    let (service, provider) = management.snapshot();
    let embedding = {
        let embedder = service.embedder();
        EmbeddingInfo {
            provider: provider.as_str().into(),
            model: embedder.model_id().into(),
            dimension: embedder.dimension(),
        }
    };
    tracing::info!(provider = %embedding.provider, model = %embedding.model, dimension = embedding.dimension, "embedding provider ready");
    let api_key = (!config.server.api_key.is_empty()).then(|| config.server.api_key.clone());
    let max_body_bytes = config.server.max_body_mb.saturating_mul(1024 * 1024);
    let state = AppState {
        service,
        api_key,
        max_body_bytes,
        request_timeout: Duration::from_secs(config.server.request_timeout_secs),
        embedding,
    };
    let app = build_managed_router(state, management.clone());

    let listener = TcpListener::bind((config.server.host.as_str(), config.server.port))
        .await
        .with_context(|| {
            format!(
                "failed to bind {}:{}",
                config.server.host, config.server.port
            )
        })?;
    let workbench = open_workbench(management, store, config.qdrant.url).await?;
    tracing::info!(address = %listener.local_addr()?, "kb-server listening");

    let (shutdown_tx, shutdown_rx) = watch::channel(false);
    let workbench_shutdown = shutdown_rx.clone();
    tokio::spawn(async move {
        shutdown_signal().await;
        shutdown_tx.send_replace(true);
    });
    tokio::try_join!(
        async {
            serve(listener, app, shutdown_rx)
                .await
                .context("HTTP server failed")
        },
        async {
            if let Some((listener, app)) = workbench {
                serve(listener, app, workbench_shutdown)
                    .await
                    .context("workbench server failed")?;
            }
            Ok(())
        },
    )?;
    tracing::info!("kb-server stopped");
    Ok(())
}

/// Enable the local, unauthenticated workbench only when explicitly requested.
async fn open_workbench(
    management: Arc<Management>,
    store: Arc<QdrantStore>,
    qdrant_url: String,
) -> Result<Option<(TcpListener, Router)>> {
    let Some(port) = env::var("DASHBOARD_PORT")
        .ok()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(None);
    };
    let port = port
        .trim()
        .parse::<u16>()
        .context("invalid DASHBOARD_PORT")?;
    let host = env::var("DASHBOARD_HOST").unwrap_or_else(|_| kb_dashboard::DEFAULT_HOST.into());
    let listener = TcpListener::bind((host.as_str(), port))
        .await
        .with_context(|| format!("failed to bind workbench to {host}:{port}"))?;
    let address = listener.local_addr()?;
    let ui_dir = kb_dashboard::resolve_ui_dir();
    tracing::info!(%address, ui_dir = %ui_dir.display(), "workbench listening");
    if !ui_dir.join("index.html").is_file() {
        tracing::warn!(
            "workbench bundle not found; run trunk build --release in crates/kb-dashboard-ui"
        );
    }
    let state = kb_dashboard::AppState::new(store, qdrant_url, move || management.snapshot());
    Ok(Some((listener, kb_dashboard::build_app(state, ui_dir))))
}

async fn serve(
    listener: TcpListener,
    app: Router,
    mut shutdown: watch::Receiver<bool>,
) -> std::io::Result<()> {
    axum::serve(listener, app)
        .with_graceful_shutdown(async move {
            let _ = shutdown.wait_for(|stopped| *stopped).await;
        })
        .await
}

/// Parses `--config <path>` / `--config=<path>`; `KB_CONFIG` is honored by
/// [`AppConfig::load`] when no path is supplied.
fn parse_config_path() -> Result<Option<PathBuf>> {
    let mut args = std::env::args().skip(1);
    while let Some(argument) = args.next() {
        if argument == "--config" {
            let value = args
                .next()
                .ok_or_else(|| anyhow::anyhow!("--config requires a path argument"))?;
            return Ok(Some(PathBuf::from(value)));
        }
        if let Some(value) = argument.strip_prefix("--config=") {
            if value.is_empty() {
                bail!("--config requires a path argument");
            }
            return Ok(Some(PathBuf::from(value)));
        }
    }
    Ok(None)
}

/// Initializes tracing with `RUST_LOG`, defaulting to `info,kb_core=debug`.
fn init_tracing() {
    let filter =
        EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info,kb_core=debug"));
    let _ = tracing_subscriber::fmt().with_env_filter(filter).try_init();
}

/// Resolves on Ctrl-C, and on SIGTERM when built for Unix.
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
