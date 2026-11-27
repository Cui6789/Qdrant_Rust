//! Library surface of the knowledge base HTTP server.
//!
//! The binary in `main.rs` uses these modules to assemble and run the service;
//! integration tests import them directly to drive the router in-process.

pub mod api;
pub mod error;
pub mod management;
pub mod state;

pub use error::ApiError;
pub use state::AppState;

use std::sync::Arc;

use kb_core::config::{EmbeddingConfig, EmbeddingProtocol, ProviderKind};
use kb_core::embedding::api::ApiEmbedding;
use kb_core::embedding::dashscope::DashScopeEmbedding;
#[cfg(feature = "local-embedding")]
use kb_core::embedding::local::LocalEmbedding;
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::embedding::EmbeddingProvider;
use kb_core::error::{KbError, Result};

/// Builds the embedding provider selected by configuration.
///
/// The local provider is initialized on a blocking thread because fastembed may
/// download or load an ONNX model during construction.
pub async fn build_embedder(config: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>> {
    match config.provider {
        ProviderKind::Api => match config.api.protocol {
            EmbeddingProtocol::Openai => Ok(Arc::new(
                ApiEmbedding::new(
                    &config.api,
                    config.batch_size,
                    config.max_concurrent_batches,
                )
                .await?,
            )),
            EmbeddingProtocol::Dashscope => Ok(Arc::new(
                DashScopeEmbedding::new(&config.api, config.batch_size).await?,
            )),
        },
        ProviderKind::Local => build_local_embedder(config).await,
        ProviderKind::Mock => Ok(Arc::new(DeterministicEmbedding::new(
            config.mock_dimensions,
        ))),
    }
}

#[cfg(feature = "local-embedding")]
async fn build_local_embedder(config: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>> {
    let local = config.local.clone();
    let batch_size = config.batch_size;
    let provider = tokio::task::spawn_blocking(move || LocalEmbedding::new(&local, batch_size))
        .await
        .map_err(|error| {
            KbError::Embedding(format!(
                "local embedding initialization task failed: {error}"
            ))
        })??;
    Ok(Arc::new(provider))
}

#[cfg(not(feature = "local-embedding"))]
async fn build_local_embedder(_config: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>> {
    Err(KbError::Config(
        "local-embedding feature is not enabled".to_string(),
    ))
}
