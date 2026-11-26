//! Shared state for the dashboard server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use anyhow::Context;
use chrono::{DateTime, Utc};
use kb_core::config::{AppConfig, IngestConfig};
use kb_core::embedding::EmbeddingProvider;
use kb_core::store::QdrantStore;
use kb_dashboard_types::{EmbeddingDefaults, EmbeddingStatus, PipelineJob, PipelineResult};

/// Default number of Qdrant points scanned when building the document list.
pub const DEFAULT_DOC_SCAN_LIMIT: u32 = 2_000;
/// Hard upper bound accepted from the UI.
pub const MAX_DOC_SCAN_LIMIT: u32 = 10_000;
/// Number of recent ingestion jobs kept in memory.
const MAX_JOBS: usize = 40;

/// Everything an HTTP handler needs.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<QdrantStore>,
    pub qdrant_url: String,
    pub qdrant_api_key_configured: bool,
    pub ingest: IngestConfig,
    pub embedding: Arc<tokio::sync::RwLock<EmbeddingRuntime>>,
    pub defaults: EmbeddingDefaults,
    pub embedding_timeout_secs: u64,
    pub embedding_max_retries: usize,
    pub embedding_batch_size: usize,
    pub embedding_max_concurrent_batches: usize,
    pub jobs: Arc<Mutex<JobStore>>,
    pub started_instant: Instant,
    pub started_at: DateTime<Utc>,
    pub version: String,
}

impl AppState {
    /// Creates dashboard state from the shared application config.
    ///
    /// The Qdrant client is constructed here, but no connection is attempted;
    /// the caller decides whether to probe it during startup.
    pub fn from_config(config: &AppConfig) -> anyhow::Result<Self> {
        let store = Arc::new(QdrantStore::new(&config.qdrant).context("创建 Qdrant 客户端失败")?);
        Ok(Self {
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
        })
    }

    /// Cloneable snapshot of the currently configured embedding status.
    pub async fn embedding_status(&self) -> EmbeddingStatus {
        self.embedding.read().await.status.clone()
    }

    /// Provider snapshot; `None` when no embedding endpoint is configured.
    pub async fn embedding_provider(&self) -> Option<Arc<dyn EmbeddingProvider>> {
        self.embedding.read().await.provider.clone()
    }
}

/// Mutable embedding configuration held in memory for this process only.
pub struct EmbeddingRuntime {
    pub provider: Option<Arc<dyn EmbeddingProvider>>,
    pub status: EmbeddingStatus,
}

impl Default for EmbeddingRuntime {
    fn default() -> Self {
        Self {
            provider: None,
            status: EmbeddingStatus::empty(),
        }
    }
}

/// In-memory registry of ingestion jobs, bounded to the newest [`MAX_JOBS`].
#[derive(Default)]
pub struct JobStore {
    jobs: HashMap<String, PipelineJob>,
    order: Vec<String>,
}

impl JobStore {
    /// Inserts a job and evicts the oldest entries beyond the retention bound.
    pub fn insert(&mut self, job: PipelineJob) {
        let id = job.id.clone();
        self.jobs.insert(id.clone(), job);
        self.order.push(id);
        while self.order.len() > MAX_JOBS {
            let oldest = self.order.remove(0);
            self.jobs.remove(&oldest);
        }
    }

    /// Returns a snapshot of a job.
    pub fn get(&self, id: &str) -> Option<PipelineJob> {
        self.jobs.get(id).cloned()
    }

    /// Mutates a job in place if it still exists.
    pub fn update(&mut self, id: &str, mutate: impl FnOnce(&mut PipelineJob)) {
        if let Some(job) = self.jobs.get_mut(id) {
            mutate(job);
        }
    }

    /// Stores the successful result and marks the job as finished.
    pub fn finish_success(&mut self, id: &str, result: PipelineResult, finished_at: String) {
        self.update(id, |job| {
            job.status = kb_dashboard_types::JobStatus::Succeeded;
            job.finished_at = Some(finished_at);
            job.result = Some(result);
            job.error = None;
        });
    }

    /// Stores a failure message and marks the job as finished.
    pub fn finish_failure(&mut self, id: &str, error: String, finished_at: String) {
        self.update(id, |job| {
            job.status = kb_dashboard_types::JobStatus::Failed;
            job.finished_at = Some(finished_at);
            job.error = Some(error);
        });
    }
}
