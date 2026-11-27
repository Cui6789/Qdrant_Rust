//! HTTP handlers for the visual dashboard API.
//!
//! Operations that depend on the active model use the host's knowledge-base
//! service snapshot. The dashboard never manages model settings.

use std::collections::{BTreeMap, HashSet};
use std::time::{Duration, Instant};

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::Json;
use chrono::{SecondsFormat, Utc};
use kb_core::error::KbError;
use kb_core::models::{DocumentFormat, DocumentMetadata, SearchRequest as CoreSearchRequest};
use kb_core::service::{validate_collection_name, IngestInput};
use kb_dashboard_types::*;
use qdrant_client::qdrant::r#match::MatchValue;
use qdrant_client::qdrant::{Condition, Filter};
use serde::Deserialize;
use serde_json::{json, Map, Value};
use uuid::Uuid;

use crate::error::ApiError;
use crate::pipeline::{self, PreparedPipeline};
use crate::state::{AppState, DEFAULT_DOC_SCAN_LIMIT, MAX_DOC_SCAN_LIMIT};

/// Handler result alias.
type ApiResult<T> = Result<T, ApiError>;

/// Qdrant timeout for overview/health probes so a hung server cannot stall the UI.
const PROBE_TIMEOUT: Duration = Duration::from_secs(3);
/// Maximum chunks returned for one document detail request.
const DOCUMENT_CHUNK_LIMIT: u32 = 5_000;

/// Query parameters of `GET /api/collections/{name}/documents`.
#[derive(Debug, Deserialize)]
pub struct ListDocumentsQuery {
    #[serde(default)]
    pub q: Option<String>,
    #[serde(default)]
    pub limit: Option<u32>,
}

/// Query parameters of the document detail/delete endpoints.
#[derive(Debug, Deserialize)]
pub struct DocumentQuery {
    pub doc_id: String,
}

/// `GET /api/health`.
pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<Value>) {
    let (qdrant, _) = collections_and_status(&state).await;
    let embedding = state.embedding_status();
    let status = if qdrant.ready { "ok" } else { "degraded" };
    let code = if qdrant.ready {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(json!({
            "status": status,
            "qdrant": qdrant,
            "embedding": embedding,
            "server": server_info(&state),
        })),
    )
}

/// `GET /api/overview`.
pub async fn overview(State(state): State<AppState>) -> Json<OverviewResponse> {
    let (qdrant, collections) = collections_and_status(&state).await;
    let embedding = state.embedding_status();
    let total_points = collections.iter().map(|item| item.points_count).sum();
    let max_dimension = collections
        .iter()
        .map(|item| item.vector_size)
        .max()
        .unwrap_or(0);
    Json(OverviewResponse {
        qdrant,
        embedding,
        collections,
        server: server_info(&state),
        scanned_at: now_ms(),
        total_points,
        max_dimension,
    })
}

/// `GET /api/collections`.
pub async fn list_collections(State(state): State<AppState>) -> ApiResult<Json<Value>> {
    let collections = state.store.list_collections().await?;
    let collections: Vec<CollectionSummary> = collections
        .into_iter()
        .map(|item| CollectionSummary {
            name: item.name,
            points_count: item.points_count,
            vector_size: item.vector_size,
        })
        .collect();
    Ok(Json(json!({ "collections": collections })))
}

/// `POST /api/collections`.
pub async fn create_collection(
    State(state): State<AppState>,
    payload: Result<Json<CreateCollectionRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<CollectionSummary>)> {
    let Json(request) = payload.map_err(json_rejection)?;
    let collection = state.service().create_collection(&request.name).await?;
    Ok((
        StatusCode::CREATED,
        Json(CollectionSummary {
            name: collection.name,
            points_count: collection.points_count,
            vector_size: collection.vector_size,
        }),
    ))
}

/// `DELETE /api/collections/{name}`.
pub async fn delete_collection(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<DeleteCollectionResponse>> {
    state.service().delete_collection(&name).await?;
    Ok(Json(DeleteCollectionResponse {
        name,
        deleted: true,
    }))
}

/// `GET /api/collections/{name}/documents`.
pub async fn list_documents(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    query: Result<Query<ListDocumentsQuery>, QueryRejection>,
) -> ApiResult<Json<DocumentListResponse>> {
    let Query(query) = query.map_err(query_rejection)?;
    let info = state.store.collection_info(&collection).await?;
    let limit = query
        .limit
        .unwrap_or(DEFAULT_DOC_SCAN_LIMIT)
        .clamp(100, MAX_DOC_SCAN_LIMIT);
    let points = state.store.scroll_points(&collection, None, limit).await?;
    let scanned_points = points.len() as u64;

    let mut documents: BTreeMap<String, DocumentAccumulator> = BTreeMap::new();
    for point in &points {
        let doc_id = payload_str(&point.payload, "doc_id").unwrap_or("(unknown)");
        let entry = documents
            .entry(doc_id.to_string())
            .or_insert_with(|| DocumentAccumulator::new(doc_id));
        entry.observe(&point.payload);
    }

    let needle = query
        .q
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty())
        .map(str::to_lowercase);
    let mut summaries: Vec<DocumentSummary> = documents
        .into_values()
        .map(DocumentAccumulator::finish)
        .filter(|document| match &needle {
            Some(needle) => document_matches(document, needle),
            None => true,
        })
        .collect();
    summaries.sort_by(|left, right| {
        right
            .created_at
            .cmp(&left.created_at)
            .then_with(|| left.doc_id.cmp(&right.doc_id))
    });

    Ok(Json(DocumentListResponse {
        collection,
        vector_size: info.vector_size,
        points_count: info.points_count,
        documents: summaries,
        scanned_points,
        partial: scanned_points >= limit as u64 || scanned_points < info.points_count,
        scanned_at: now_ms(),
        limit,
    }))
}

/// `GET /api/collections/{name}/document?doc_id=...`.
pub async fn document_detail(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    query: Result<Query<DocumentQuery>, QueryRejection>,
) -> ApiResult<Json<DocumentDetailResponse>> {
    let Query(query) = query.map_err(query_rejection)?;
    state.store.collection_info(&collection).await?;
    let points = state
        .store
        .scroll_points(
            &collection,
            Some(doc_filter(&query.doc_id)),
            DOCUMENT_CHUNK_LIMIT,
        )
        .await?;
    if points.is_empty() {
        return Err(ApiError::not_found(format!(
            "文档 {} 在集合 {} 中不存在",
            query.doc_id, collection
        )));
    }

    let mut chunks: Vec<ChunkDetail> = points
        .iter()
        .map(|point| ChunkDetail {
            point_id: point.id.clone(),
            chunk_index: payload_u64(&point.payload, "chunk_index").unwrap_or(0),
            text: payload_str(&point.payload, "text")
                .unwrap_or_default()
                .to_string(),
            heading_path: payload_str(&point.payload, "heading_path")
                .unwrap_or_default()
                .to_string(),
            characters: payload_str(&point.payload, "text")
                .map(|text| text.chars().count() as u64)
                .unwrap_or(0),
            created_at: payload_str(&point.payload, "created_at").map(str::to_string),
        })
        .collect();
    chunks.sort_by_key(|chunk| chunk.chunk_index);

    let first = &points[0].payload;
    let characters = chunks.iter().map(|chunk| chunk.characters).sum();
    let chunk_count = points
        .iter()
        .filter_map(|point| payload_u64(&point.payload, "chunk_count"))
        .max()
        .unwrap_or(chunks.len() as u64)
        .max(chunks.len() as u64);

    Ok(Json(DocumentDetailResponse {
        collection,
        doc_id: query.doc_id,
        title: payload_str(first, "title").map(str::to_string),
        source: payload_str(first, "source").map(str::to_string),
        tags: payload_strings(first, "tags"),
        extra: payload_extra(first),
        embedding_model: payload_str(first, "embedding_model").map(str::to_string),
        content_hash: payload_str(first, "content_hash").map(str::to_string),
        chunk_count,
        characters,
        chunks,
    }))
}

/// `DELETE /api/collections/{name}/document?doc_id=...`.
pub async fn delete_document(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    query: Result<Query<DocumentQuery>, QueryRejection>,
) -> ApiResult<Json<DeleteDocumentResponse>> {
    let Query(query) = query.map_err(query_rejection)?;
    let deleted_points = state
        .service()
        .delete_document(&collection, &query.doc_id)
        .await?;
    Ok(Json(DeleteDocumentResponse {
        doc_id: query.doc_id,
        deleted_points,
    }))
}

/// `POST /api/collections/{name}/search`.
pub async fn search(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    payload: Result<Json<SearchRequest>, JsonRejection>,
) -> ApiResult<Json<SearchResponse>> {
    let Json(request) = payload.map_err(json_rejection)?;
    let query = request.query.trim();
    if query.is_empty() {
        return Err(ApiError::validation("检索内容不能为空"));
    }
    let service = state.service();
    let top_k = request.top_k.unwrap_or(5).clamp(1, 50);
    let started = Instant::now();
    let hits = service
        .search(
            &collection,
            CoreSearchRequest {
                query: query.to_string(),
                top_k,
                score_threshold: request.score_threshold,
                filter: None,
            },
        )
        .await?;
    let results = hits
        .into_iter()
        .map(|hit| SearchHit {
            score: hit.score,
            rerank_score: hit.rerank_score,
            doc_id: hit.doc_id,
            chunk_index: hit.chunk_index,
            text: hit.text,
            title: hit.metadata.title,
            source: hit.metadata.source,
            tags: hit.metadata.tags,
            heading_path: hit.metadata.heading_path,
        })
        .collect();
    Ok(Json(SearchResponse {
        query: query.to_string(),
        embedding_model: service.embedder().model_id().to_string(),
        results,
        took_ms: started.elapsed().as_millis() as u64,
    }))
}

/// `POST /api/pipeline/run`.
pub async fn run_pipeline(
    State(state): State<AppState>,
    payload: Result<Json<PipelineRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<PipelineStartResponse>)> {
    let Json(request) = payload.map_err(json_rejection)?;
    if request.text.trim().is_empty() {
        return Err(ApiError::validation("文档内容不能为空"));
    }
    validate_collection_name(&request.collection)?;
    let format = match request.format.as_deref() {
        Some(value) => DocumentFormat::parse(value)?,
        None => DocumentFormat::Text,
    };
    let extra = match request.extra {
        Value::Null => Map::new(),
        Value::Object(map) => map,
        _ => return Err(ApiError::validation("extra 必须是 JSON 对象")),
    };
    let metadata = DocumentMetadata {
        source: clean_optional(request.source),
        title: clean_optional(request.title),
        tags: normalize_tags(request.tags),
        extra,
    };
    let doc_id = clean_optional(request.doc_id);
    let service = state.service();
    let dimension = service.embedder().dimension() as u64;

    let create_if_missing = request.create_collection.unwrap_or(true);
    let info = match service.collection_info(&request.collection).await {
        Err(KbError::CollectionNotFound(_)) if create_if_missing => {
            match service.create_collection(&request.collection).await {
                Err(KbError::CollectionExists(_)) => {
                    service.collection_info(&request.collection).await?
                }
                result => result?,
            }
        }
        result => result?,
    };
    if info.vector_size != dimension {
        return Err(ApiError::conflict(
            "EMBEDDING_DIM_MISMATCH",
            format!(
                "集合 {} 使用 {} 维向量，而当前嵌入模型输出 {} 维；请新建集合并重新写入",
                request.collection, info.vector_size, dimension
            ),
        ));
    }

    let points_before = info.points_count;
    let job_id = Uuid::new_v4().to_string();
    let started = Instant::now();
    let started_at = now_ms();
    let job = PipelineJob {
        id: job_id.clone(),
        status: JobStatus::Queued,
        collection: request.collection.clone(),
        doc_id_hint: doc_id.clone(),
        started_at,
        finished_at: None,
        events: Vec::new(),
        result: None,
        error: None,
    };
    {
        let mut jobs = state
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.insert(job);
    }

    pipeline::spawn(
        state,
        PreparedPipeline {
            job_id: job_id.clone(),
            collection: request.collection,
            input: IngestInput {
                doc_id,
                text: request.text,
                format,
                metadata,
            },
            points_before,
            service,
            started,
        },
    );

    Ok((StatusCode::ACCEPTED, Json(PipelineStartResponse { job_id })))
}

/// `GET /api/pipeline/jobs/{id}`.
pub async fn get_pipeline_job(
    State(state): State<AppState>,
    Path(id): Path<String>,
) -> ApiResult<Json<PipelineJob>> {
    let job = {
        let jobs = state
            .jobs
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        jobs.get(&id)
    };
    job.map(Json)
        .ok_or_else(|| ApiError::not_found("任务不存在或已过期（服务只保留最近 40 个任务）"))
}

// ---------------------------------------------------------------------------
// Helpers
// ---------------------------------------------------------------------------

/// Accumulates per-document statistics while scanning point payloads.
struct DocumentAccumulator {
    doc_id: String,
    title: Option<String>,
    source: Option<String>,
    tags: Vec<String>,
    embedding_model: Option<String>,
    created_at: Option<String>,
    content_hash: Option<String>,
    chunk_count: u64,
    scanned_chunks: u64,
    characters: u64,
    heading_paths: Vec<String>,
    extra: Value,
}

impl DocumentAccumulator {
    fn new(doc_id: &str) -> Self {
        Self {
            doc_id: doc_id.to_string(),
            title: None,
            source: None,
            tags: Vec::new(),
            embedding_model: None,
            created_at: None,
            content_hash: None,
            chunk_count: 0,
            scanned_chunks: 0,
            characters: 0,
            heading_paths: Vec::new(),
            extra: Value::Null,
        }
    }

    fn observe(&mut self, payload: &Map<String, Value>) {
        self.scanned_chunks += 1;
        self.characters += payload_str(payload, "text")
            .map(|text| text.chars().count() as u64)
            .unwrap_or(0);
        self.chunk_count = self
            .chunk_count
            .max(payload_u64(payload, "chunk_count").unwrap_or(0));
        if self.title.is_none() {
            self.title = payload_str(payload, "title").map(str::to_string);
        }
        if self.source.is_none() {
            self.source = payload_str(payload, "source").map(str::to_string);
        }
        if self.tags.is_empty() {
            self.tags = payload_strings(payload, "tags");
        }
        if self.embedding_model.is_none() {
            self.embedding_model = payload_str(payload, "embedding_model").map(str::to_string);
        }
        if self.content_hash.is_none() {
            self.content_hash = payload_str(payload, "content_hash").map(str::to_string);
        }
        if let Some(created_at) = payload_str(payload, "created_at") {
            if self
                .created_at
                .as_deref()
                .is_none_or(|current| created_at > current)
            {
                self.created_at = Some(created_at.to_string());
            }
        }
        if let Some(heading) = payload_str(payload, "heading_path") {
            if !heading.is_empty()
                && !self.heading_paths.iter().any(|current| current == heading)
                && self.heading_paths.len() < 6
            {
                self.heading_paths.push(heading.to_string());
            }
        }
        if self.extra.is_null() {
            self.extra = payload_extra(payload);
        }
    }

    fn finish(mut self) -> DocumentSummary {
        if self.chunk_count < self.scanned_chunks {
            self.chunk_count = self.scanned_chunks;
        }
        if self.extra.is_null() {
            self.extra = Value::Object(Map::new());
        }
        DocumentSummary {
            doc_id: self.doc_id,
            title: self.title,
            source: self.source,
            tags: self.tags,
            embedding_model: self.embedding_model,
            created_at: self.created_at,
            content_hash: self.content_hash,
            chunk_count: self.chunk_count,
            scanned_chunks: self.scanned_chunks,
            characters: self.characters,
            heading_paths: self.heading_paths,
            extra: self.extra,
        }
    }
}

fn document_matches(document: &DocumentSummary, needle: &str) -> bool {
    document.doc_id.to_lowercase().contains(needle)
        || document
            .title
            .as_deref()
            .map(|title| title.to_lowercase().contains(needle))
            .unwrap_or(false)
        || document
            .source
            .as_deref()
            .map(|source| source.to_lowercase().contains(needle))
            .unwrap_or(false)
        || document
            .tags
            .iter()
            .any(|tag| tag.to_lowercase().contains(needle))
}

fn payload_str<'a>(payload: &'a Map<String, Value>, key: &str) -> Option<&'a str> {
    payload.get(key).and_then(Value::as_str)
}

fn payload_u64(payload: &Map<String, Value>, key: &str) -> Option<u64> {
    payload.get(key).and_then(|value| match value {
        Value::Number(number) => number.as_u64(),
        _ => None,
    })
}

fn payload_strings(payload: &Map<String, Value>, key: &str) -> Vec<String> {
    payload
        .get(key)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

fn payload_extra(payload: &Map<String, Value>) -> Value {
    payload
        .get("extra")
        .cloned()
        .unwrap_or_else(|| Value::Object(Map::new()))
}

fn doc_filter(doc_id: &str) -> Filter {
    Filter::must([Condition::matches(
        "doc_id",
        MatchValue::Keyword(doc_id.to_string()),
    )])
}

async fn collections_and_status(state: &AppState) -> (QdrantStatus, Vec<CollectionSummary>) {
    let started = Instant::now();
    match tokio::time::timeout(PROBE_TIMEOUT, state.store.list_collections()).await {
        Ok(Ok(collections)) => {
            let latency_ms = started.elapsed().as_millis() as u64;
            (
                QdrantStatus {
                    url: state.qdrant_url.clone(),
                    ready: true,
                    latency_ms: Some(latency_ms),
                    error: None,
                },
                collections
                    .into_iter()
                    .map(|item| CollectionSummary {
                        name: item.name,
                        points_count: item.points_count,
                        vector_size: item.vector_size,
                    })
                    .collect(),
            )
        }
        Ok(Err(error)) => (
            QdrantStatus {
                url: state.qdrant_url.clone(),
                ready: false,
                latency_ms: Some(started.elapsed().as_millis() as u64),
                error: Some(error.to_string()),
            },
            Vec::new(),
        ),
        Err(_) => (
            QdrantStatus {
                url: state.qdrant_url.clone(),
                ready: false,
                latency_ms: Some(PROBE_TIMEOUT.as_millis() as u64),
                error: Some(format!("Qdrant 在 {} 秒内未响应", PROBE_TIMEOUT.as_secs())),
            },
            Vec::new(),
        ),
    }
}

fn server_info(state: &AppState) -> ServerInfo {
    ServerInfo {
        version: state.version.clone(),
        uptime_secs: state.started_instant.elapsed().as_secs(),
        started_at: state.started_at.to_rfc3339_opts(SecondsFormat::Secs, true),
        poll_hint_ms: 3000,
    }
}

fn clean_optional(value: Option<String>) -> Option<String> {
    value
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
}

fn normalize_tags(tags: Vec<String>) -> Vec<String> {
    let mut seen = HashSet::new();
    tags.into_iter()
        .map(|tag| tag.trim().to_string())
        .filter(|tag| !tag.is_empty())
        .filter(|tag| seen.insert(tag.clone()))
        .collect()
}

fn now_ms() -> String {
    Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true)
}

fn json_rejection(rejection: JsonRejection) -> ApiError {
    ApiError::validation(rejection.body_text())
}

fn query_rejection(rejection: QueryRejection) -> ApiError {
    ApiError::validation(rejection.body_text())
}
