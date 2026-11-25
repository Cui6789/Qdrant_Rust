use std::sync::atomic::{AtomicUsize, Ordering};

use sha2::{Digest, Sha256};

use super::EmbeddingProvider;
use crate::error::Result;

/// Deterministic embedding provider for tests and demos.
///
/// Vectors are derived from a SHA-256 digest of the input text, expanded into
/// `f32` values in `[-1.0, 1.0]` and L2-normalized. Identical text always maps
/// to an identical vector, so this provider never needs the network or a model
/// download.
pub struct DeterministicEmbedding {
    dim: usize,
    calls: AtomicUsize,
}

impl DeterministicEmbedding {
    /// Creates a provider producing vectors with `dim` dimensions.
    pub fn new(dim: usize) -> Self {
        Self {
            dim,
            calls: AtomicUsize::new(0),
        }
    }

    /// Returns how many times [`EmbeddingProvider::embed_documents`] was called.
    pub fn call_count(&self) -> usize {
        self.calls.load(Ordering::SeqCst)
    }

    fn vector_for(&self, text: &str) -> Vec<f32> {
        let mut vector = Vec::with_capacity(self.dim);
        let mut block = Sha256::digest(text.as_bytes()).to_vec();
        while vector.len() < self.dim {
            for byte in &block {
                if vector.len() == self.dim {
                    break;
                }
                vector.push(f32::from(*byte) / 127.5 - 1.0);
            }
            block = Sha256::digest(&block).to_vec();
        }
        let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
        if norm > 0.0 {
            for value in &mut vector {
                *value /= norm;
            }
        }
        vector
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for DeterministicEmbedding {
    fn model_id(&self) -> &str {
        "deterministic-mock"
    }

    fn dimension(&self) -> usize {
        self.dim
    }

    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(texts.iter().map(|text| self.vector_for(text)).collect())
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let texts = [text.to_string()];
        let mut vectors = self.embed_documents(&texts).await?;
        Ok(vectors.remove(0))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::embedding::EmbeddingProvider;

    #[tokio::test]
    async fn same_text_yields_identical_vector() {
        let provider = DeterministicEmbedding::new(64);
        let first = provider
            .embed_documents(&["同一个句子".to_string()])
            .await
            .unwrap();
        let second = provider
            .embed_documents(&["同一个句子".to_string()])
            .await
            .unwrap();
        assert_eq!(first, second);
        assert_eq!(first[0].len(), 64);
    }

    #[tokio::test]
    async fn dimension_matches_vector_length() {
        let provider = DeterministicEmbedding::new(16);
        let vectors = provider
            .embed_documents(&["hello".to_string(), "world".to_string()])
            .await
            .unwrap();
        assert_eq!(provider.dimension(), 16);
        assert_eq!(vectors.len(), 2);
        assert!(vectors
            .iter()
            .all(|vector| vector.len() == provider.dimension()));
    }

    #[tokio::test]
    async fn different_texts_differ() {
        let provider = DeterministicEmbedding::new(32);
        let vectors = provider
            .embed_documents(&["alpha".to_string(), "beta".to_string()])
            .await
            .unwrap();
        assert_ne!(vectors[0], vectors[1]);
    }

    #[tokio::test]
    async fn call_count_increments_once_per_embed_documents() {
        let provider = DeterministicEmbedding::new(8);
        assert_eq!(provider.call_count(), 0);
        provider
            .embed_documents(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();
        assert_eq!(provider.call_count(), 1);
        provider.embed_documents(&["c".to_string()]).await.unwrap();
        assert_eq!(provider.call_count(), 2);
    }
}
