pub mod api;
pub mod dashscope;
#[cfg(feature = "local-embedding")]
pub mod local;
pub mod mock;

use crate::error::Result;

/// A pluggable text embedding provider.
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    /// Identifier of the model backing this provider.
    fn model_id(&self) -> &str;

    /// Dimensionality of the vectors produced by this provider.
    fn dimension(&self) -> usize;

    /// Embeds a batch of documents, preserving input order.
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;

    /// Embeds a single search query.
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}
