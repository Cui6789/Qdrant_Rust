//! Shared state for the dashboard server.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{DateTime, Utc};
use kb_core::config::ProviderKind;
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;
use kb_dashboard_types::{EmbeddingStatus, PipelineJob, PipelineResult};

/// Default number of Qdrant points scanned when building the document list.
pub const DEFAULT_DOC_SCAN_LIMIT: u32 = 2_000;
/// Hard upper bound accepted from the UI.
pub const MAX_DOC_SCAN_LIMIT: u32 = 10_000;
/// Number of recent ingestion jobs kept in memory.
const MAX_JOBS: usize = 40;

type CurrentService = dyn Fn() -> (Arc<KnowledgeBaseService>, ProviderKind) + Send + Sync;

/// Everything an HTTP handler needs.
#[derive(Clone)]
pub struct AppState {
    pub store: Arc<QdrantStore>,
    pub qdrant_url: String,
    current_service: Arc<CurrentService>,
    pub jobs: Arc<Mutex<JobStore>>,
    pub started_instant: Instant,
    pub started_at: DateTime<Utc>,
    pub version: String,
}

impl AppState {
    /// Uses the knowledge-base service snapshot supplied by the host on each request.
    pub fn new(
        store: Arc<QdrantStore>,
        qdrant_url: String,
        current_service: impl Fn() -> (Arc<KnowledgeBaseService>, ProviderKind) + Send + Sync + 'static,
    ) -> Self {
        Self {
            store,
            qdrant_url,
            current_service: Arc::new(current_service),
            jobs: Arc::new(Mutex::new(JobStore::default())),
            started_instant: Instant::now(),
            started_at: Utc::now(),
            version: env!("CARGO_PKG_VERSION").to_string(),
        }
    }

    pub fn embedding_status(&self) -> EmbeddingStatus {
        let (service, kind) = (self.current_service)();
        let embedder = service.embedder();
        EmbeddingStatus {
            configured: true,
            kind: Some(kind.as_str().into()),
            model: Some(embedder.model_id().into()),
            dimension: Some(embedder.dimension()),
        }
    }

    pub fn service(&self) -> Arc<KnowledgeBaseService> {
        (self.current_service)().0
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
