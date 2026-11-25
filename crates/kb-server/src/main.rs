//! Knowledge base HTTP server entry point.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{bail, Context, Result};
use kb_core::config::{AppConfig, ProviderKind};
use kb_core::models::EmbeddingInfo;
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;
use kb_server::api::build_router;
use kb_server::build_embedder;
use kb_server::state::AppState;
use tokio::net::TcpListener;
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
        provider = provider_label(config.embedding.provider),
        auth_enabled = !config.server.api_key.is_empty(),
        max_body_mb = config.server.max_body_mb,
        request_timeout_secs = config.server.request_timeout_secs,
        "starting kb-server"
    );

    // `chunk_size` is measured in Unicode characters while `max_length` is a
    // model token limit; one CJK character is often close to one token, so a
    // chunk larger than the window is silently truncated by the model.
    if config.embedding.provider == ProviderKind::Local
        && config.ingest.chunk_size > config.embedding.local.max_length
    {
        tracing::warn!(
            chunk_size = config.ingest.chunk_size,
            max_length = config.embedding.local.max_length,
            "ingest.chunk_size counts Unicode characters but embedding.local.max_length counts model tokens \
             (a rough characters-to-tokens approximation, not an exact conversion); longer chunks are silently \
             truncated. Lower ingest.chunk_size (for example 400) for CJK models such as bge-small-zh-v1.5."
        );
    }

    let store = QdrantStore::new(&config.qdrant).context("failed to build the Qdrant client")?;
    store
        .wait_ready(Duration::from_secs(config.qdrant.startup_retry_secs))
        .await
        .context("Qdrant is not ready")?;

    let embedder = build_embedder(&config.embedding)
        .await
        .context("failed to initialize the embedding provider")?;
    let dimension = embedder.dimension();
    tracing::info!(
        provider = provider_label(config.embedding.provider),
        model = %embedder.model_id(),
        dimension,
        "embedding provider ready"
    );

    let embedding = EmbeddingInfo {
        provider: provider_label(config.embedding.provider).to_string(),
        model: embedder.model_id().to_string(),
        dimension,
    };
    let api_key = (!config.server.api_key.is_empty()).then(|| config.server.api_key.clone());
    let max_body_bytes = config.server.max_body_mb.saturating_mul(1024 * 1024);
    let state = AppState {
        service: Arc::new(KnowledgeBaseService::new(
            Arc::new(store),
            embedder,
            config.ingest.clone(),
        )),
        api_key,
        max_body_bytes,
        request_timeout: Duration::from_secs(config.server.request_timeout_secs),
        embedding,
    };
    let app = build_router(state);

    let address = format!("{}:{}", config.server.host, config.server.port);
    let listener = TcpListener::bind(&address)
        .await
        .with_context(|| format!("failed to bind {address}"))?;
    tracing::info!(address = %address, "kb-server listening");

    axum::serve(listener, app)
        .with_graceful_shutdown(shutdown_signal())
        .await
        .context("HTTP server failed")?;
    tracing::info!("kb-server stopped");
    Ok(())
}

/// Config-aware provider label used in logs and health output.
fn provider_label(provider: ProviderKind) -> &'static str {
    match provider {
        ProviderKind::Local => "local",
        ProviderKind::Api => "api",
    }
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
