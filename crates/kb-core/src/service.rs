//! Knowledge base business logic.
//!
//! Writes for one document are serialized by striped locks. Every ingest first
//! reconciles the existing points for `doc_id + content_hash`; only a complete,
//! identical chunk set skips embedding. Retrieval owns vector search, optional
//! reranking and cursor-based browsing, while payload construction and parsing
//! live behind a shared module used by both paths.

mod payload;
mod retrieval;
mod write;

#[cfg(test)]
mod tests;

use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::config::IngestConfig;
use crate::embedding::EmbeddingProvider;
use crate::error::{KbError, Result};
use crate::ingest::upload::{UploadInput, UploadParser};
use crate::models::{CollectionInfo, EmbeddingInfo, HealthStatus, IngestResult};
use crate::rerank::RerankProvider;
use crate::store::QdrantStore;

pub use payload::{validate_collection_name, validate_search_request};
pub use write::{point_id, resolve_doc_id, ChunkPreview, IngestEvent, IngestInput};

/// Number of striped mutexes used to serialize per-document writes.
const LOCK_STRIPES: usize = 64;

/// In-process knowledge base operations over a Qdrant store.
pub struct KnowledgeBaseService {
    store: Arc<QdrantStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    reranker: Option<Arc<dyn RerankProvider>>,
    candidate_limit: u64,
    ingest: IngestConfig,
    upload: Arc<UploadParser>,
    locks: Arc<StripedLocks>,
}

impl KnowledgeBaseService {
    /// Builds a service from its collaborators.
    pub fn new(
        store: Arc<QdrantStore>,
        embedder: Arc<dyn EmbeddingProvider>,
        ingest: IngestConfig,
    ) -> Self {
        Self {
            store,
            embedder,
            reranker: None,
            candidate_limit: 0,
            ingest,
            upload: Arc::new(UploadParser::default()),
            locks: Arc::new(StripedLocks::new()),
        }
    }

    /// Creates a runtime snapshot while sharing the store and write locks.
    pub fn with_runtime(
        &self,
        embedder: Arc<dyn EmbeddingProvider>,
        reranker: Option<Arc<dyn RerankProvider>>,
        candidate_limit: u64,
        upload: Arc<UploadParser>,
    ) -> Self {
        Self {
            store: self.store.clone(),
            embedder,
            reranker,
            candidate_limit,
            ingest: self.ingest.clone(),
            upload,
            locks: self.locks.clone(),
        }
    }

    /// Returns the active embedding provider for runtime settings updates.
    pub fn embedder(&self) -> Arc<dyn EmbeddingProvider> {
        self.embedder.clone()
    }

    /// Prepares and ingests an uploaded file.
    pub async fn ingest_upload(
        &self,
        collection: &str,
        input: UploadInput,
    ) -> Result<IngestResult> {
        let input = self.prepare_upload(input).await?;
        self.ingest_document(collection, input).await
    }

    /// Runs the active upload parser without embedding or writing to the store.
    pub async fn prepare_upload(&self, input: UploadInput) -> Result<IngestInput> {
        self.upload.prepare(input).await
    }

    /// Reports Qdrant connectivity and the active embedding configuration.
    pub async fn health(&self) -> HealthStatus {
        let (status, qdrant) = match self.store.list_collections().await {
            Ok(_) => ("ok", "ok"),
            Err(_) => ("degraded", "unavailable"),
        };
        HealthStatus {
            status: status.to_string(),
            qdrant: qdrant.to_string(),
            embedding: EmbeddingInfo {
                provider: self.embedder.model_id().to_string(),
                model: self.embedder.model_id().to_string(),
                dimension: self.embedder.dimension(),
            },
        }
    }

    /// Lists collections with their real point counts and vector sizes.
    pub async fn list_collections(&self) -> Result<Vec<CollectionInfo>> {
        self.store.list_collections().await
    }

    /// Verifies that this runtime's embeddings fit the target collection.
    async fn check_collection_dimension(&self, name: &str) -> Result<()> {
        let collection = self.store.collection_info(name).await?;
        let dimension = self.embedder.dimension() as u64;
        if collection.vector_size != dimension {
            return Err(KbError::DimensionMismatch(format!(
                "collection {name} uses {} dimensions, but the active embedding produces {dimension}; create a new collection and re-ingest the documents",
                collection.vector_size
            )));
        }
        Ok(())
    }

    /// Creates a collection sized for the active embedding provider.
    pub async fn create_collection(&self, name: &str) -> Result<CollectionInfo> {
        validate_collection_name(name)?;
        if self.store.collection_exists(name).await? {
            return Err(KbError::CollectionExists(name.to_string()));
        }
        let dimension = self.embedder.dimension() as u64;
        self.store.create_collection(name, dimension).await?;
        Ok(CollectionInfo {
            name: name.to_string(),
            points_count: 0,
            vector_size: dimension,
        })
    }

    /// Deletes an existing collection.
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        if !self.store.collection_exists(name).await? {
            return Err(KbError::CollectionNotFound(name.to_string()));
        }
        self.store.delete_collection(name).await
    }
}

/// Fixed-stripe mutex set keyed by `collection + doc_id`.
struct StripedLocks {
    stripes: Vec<tokio::sync::Mutex<()>>,
}

impl StripedLocks {
    fn new() -> Self {
        Self {
            stripes: (0..LOCK_STRIPES)
                .map(|_| tokio::sync::Mutex::new(()))
                .collect(),
        }
    }

    async fn lock(&self, key: &str) -> tokio::sync::MutexGuard<'_, ()> {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let index = (hasher.finish() as usize) % self.stripes.len();
        self.stripes[index].lock().await
    }
}
