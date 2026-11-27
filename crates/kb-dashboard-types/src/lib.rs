//! Wire types shared by the dashboard server (`kb-dashboard`) and its
//! WebAssembly frontend (`kb-dashboard-ui`).
//!
//! Everything here is plain serde data so the same structs compile for native
//! and `wasm32-unknown-unknown` targets. Field names use `snake_case` on the
//! wire; the UI renders them in Chinese.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// A Qdrant collection summary.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionSummary {
    pub name: String,
    pub points_count: u64,
    pub vector_size: u64,
}

/// Connectivity details for the Qdrant gRPC endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct QdrantStatus {
    pub url: String,
    pub ready: bool,
    pub latency_ms: Option<u64>,
    pub error: Option<String>,
}

/// Runtime embedding-provider status. The API key is never returned.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingStatus {
    pub configured: bool,
    pub kind: Option<String>,
    pub model: Option<String>,
    pub dimension: Option<usize>,
}

/// Server metadata shown in the dashboard footer/status cards.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerInfo {
    pub version: String,
    pub uptime_secs: u64,
    pub started_at: String,
    pub poll_hint_ms: u64,
}

/// Response of `GET /api/overview`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OverviewResponse {
    pub qdrant: QdrantStatus,
    pub embedding: EmbeddingStatus,
    pub collections: Vec<CollectionSummary>,
    pub server: ServerInfo,
    pub scanned_at: String,
    pub total_points: u64,
    pub max_dimension: u64,
}

/// Document-level summary aggregated from Qdrant point payloads.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub doc_id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub tags: Vec<String>,
    pub embedding_model: Option<String>,
    pub created_at: Option<String>,
    pub content_hash: Option<String>,
    pub chunk_count: u64,
    pub scanned_chunks: u64,
    pub characters: u64,
    pub heading_paths: Vec<String>,
    pub extra: Value,
}

/// Response of `GET /api/collections/{name}/documents`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentListResponse {
    pub collection: String,
    pub vector_size: u64,
    pub points_count: u64,
    pub documents: Vec<DocumentSummary>,
    pub scanned_points: u64,
    pub partial: bool,
    pub scanned_at: String,
    pub limit: u32,
}

/// One stored chunk returned by the document detail endpoint.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChunkDetail {
    pub point_id: String,
    pub chunk_index: u64,
    pub text: String,
    pub heading_path: String,
    pub characters: u64,
    pub created_at: Option<String>,
}

/// Response of `GET /api/collections/{name}/document`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentDetailResponse {
    pub collection: String,
    pub doc_id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub tags: Vec<String>,
    pub extra: Value,
    pub embedding_model: Option<String>,
    pub content_hash: Option<String>,
    pub chunk_count: u64,
    pub characters: u64,
    pub chunks: Vec<ChunkDetail>,
}

/// Body of `POST /api/collections/{name}/search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default)]
    pub top_k: Option<usize>,
    #[serde(default)]
    pub score_threshold: Option<f32>,
}

/// One similarity-search hit.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f32>,
    pub doc_id: String,
    pub chunk_index: u64,
    pub text: String,
    pub title: Option<String>,
    pub source: Option<String>,
    pub tags: Vec<String>,
    pub heading_path: String,
}

/// Response of `POST /api/collections/{name}/search`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchResponse {
    pub query: String,
    pub embedding_model: String,
    pub results: Vec<SearchHit>,
    pub took_ms: u64,
}

/// Body of `POST /api/collections`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
}

/// Response of `DELETE /api/collections/{name}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteCollectionResponse {
    pub name: String,
    pub deleted: bool,
}

/// Response of `DELETE /api/collections/{name}/document`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeleteDocumentResponse {
    pub doc_id: String,
    pub deleted_points: u64,
}

/// Lifecycle state of an ingestion job.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Succeeded,
    Failed,
}

impl JobStatus {
    pub fn is_finished(self) -> bool {
        matches!(self, Self::Succeeded | Self::Failed)
    }
}

/// One timestamped pipeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineEvent {
    pub seq: u64,
    pub at: String,
    pub elapsed_ms: u64,
    pub stage: String,
    pub message: String,
    pub data: Value,
}

/// Chunk preview carried by the "chunked" pipeline event.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineChunkPreview {
    pub index: usize,
    pub characters: usize,
    pub heading_path: String,
    pub preview: String,
}

/// Final result of a successful ingestion job.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineResult {
    pub doc_id: String,
    pub chunks: usize,
    pub skipped: bool,
    pub metadata_updated: bool,
    pub content_hash: String,
    pub embedding_model: String,
    pub points_before: u64,
    pub points_after: u64,
    pub duration_ms: u64,
    pub chunk_previews: Vec<PipelineChunkPreview>,
    pub vector_preview: Vec<f32>,
    pub dimension: usize,
    pub vector_norm: f32,
}

/// Snapshot returned by `GET /api/pipeline/jobs/{id}`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineJob {
    pub id: String,
    pub status: JobStatus,
    pub collection: String,
    pub doc_id_hint: Option<String>,
    pub started_at: String,
    pub finished_at: Option<String>,
    pub events: Vec<PipelineEvent>,
    pub result: Option<PipelineResult>,
    pub error: Option<String>,
}

/// Body of `POST /api/pipeline/run`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PipelineRequest {
    pub collection: String,
    pub text: String,
    #[serde(default)]
    pub doc_id: Option<String>,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub source: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub extra: Value,
    #[serde(default)]
    pub create_collection: Option<bool>,
}

/// Response of `POST /api/pipeline/run`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PipelineStartResponse {
    pub job_id: String,
}

/// Stable error envelope used by every dashboard endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorBody {
    pub error: ApiErrorDetail,
}

/// Machine-readable error code and human-readable message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ApiErrorDetail {
    pub code: String,
    pub message: String,
}
