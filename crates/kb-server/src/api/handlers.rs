//! HTTP handlers for the knowledge base REST API.

use axum::extract::multipart::MultipartRejection;
use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Multipart, Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use kb_core::error::KbError;
use kb_core::ingest::parser::decode_bytes;
use kb_core::models::{
    CollectionInfo, DocumentFormat, DocumentMetadata, HealthStatus, IngestDocumentRequest,
    IngestResult, SearchHit, SearchRequest,
};
use kb_core::service::IngestInput;
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::state::AppState;

/// Result type used by every handler.
type ApiResult<T> = Result<T, ApiError>;

/// Body of `POST /v1/collections`.
#[derive(Debug, Deserialize)]
pub struct CreateCollectionRequest {
    pub name: String,
}

/// Query parameters of `DELETE /v1/collections/{name}/documents`.
#[derive(Debug, Deserialize)]
pub struct DeleteDocumentQuery {
    pub doc_id: String,
}

/// Maps an extractor rejection onto an [`ApiError`].
///
/// Size-limit failures must stay 413 `PAYLOAD_TOO_LARGE` (the design's error
/// contract); every other extractor rejection becomes `VALIDATION_ERROR`.
fn rejection(error: impl IntoResponse + std::fmt::Display) -> ApiError {
    let message = error.to_string();
    if error.into_response().status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError(KbError::PayloadTooLarge(message))
    } else {
        ApiError(KbError::Validation(message))
    }
}

/// `GET /health` — Qdrant connectivity and embedding details, no auth.
pub async fn health(State(state): State<AppState>) -> (StatusCode, Json<HealthStatus>) {
    let mut health = state.service.health().await;
    health.embedding = state.embedding.clone();
    let status = if health.status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (status, Json(health))
}

/// `GET /v1/collections`.
pub async fn list_collections(State(state): State<AppState>) -> ApiResult<Json<serde_json::Value>> {
    let collections = state.service.list_collections().await?;
    Ok(Json(json!({ "collections": collections })))
}

/// `POST /v1/collections`.
pub async fn create_collection(
    State(state): State<AppState>,
    payload: Result<Json<CreateCollectionRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<CollectionInfo>)> {
    let Json(request) = payload.map_err(rejection)?;
    let collection = state.service.create_collection(&request.name).await?;
    Ok((StatusCode::CREATED, Json(collection)))
}

/// `GET /v1/collections/{name}`.
pub async fn get_collection(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<Json<CollectionInfo>> {
    let collections = state.service.list_collections().await?;
    match collections.into_iter().find(|info| info.name == name) {
        Some(info) => Ok(Json(info)),
        None => Err(ApiError(KbError::CollectionNotFound(name))),
    }
}

/// `DELETE /v1/collections/{name}`.
pub async fn delete_collection(
    State(state): State<AppState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    state.service.delete_collection(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/collections/{name}/documents`.
pub async fn ingest_document(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    payload: Result<Json<IngestDocumentRequest>, JsonRejection>,
) -> ApiResult<Json<IngestResult>> {
    let Json(request) = payload.map_err(rejection)?;

    // Parse the requested format before any collection or Qdrant lookup so an
    // unknown format always yields 415, even when the store is unreachable.
    let format = match request.format.as_deref() {
        Some(value) => DocumentFormat::parse(value)?,
        None => DocumentFormat::Text,
    };

    let result = state
        .service
        .ingest_document(
            &collection,
            IngestInput {
                doc_id: request.doc_id,
                text: request.text,
                format,
                metadata: request.metadata,
            },
        )
        .await?;
    Ok(Json(result))
}

/// `POST /v1/collections/{name}/documents/upload`.
pub async fn upload_document(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    multipart: Result<Multipart, MultipartRejection>,
) -> ApiResult<Json<IngestResult>> {
    let mut multipart = multipart.map_err(rejection)?;

    let mut file: Option<Vec<u8>> = None;
    let mut file_name: Option<String> = None;
    let mut doc_id: Option<String> = None;
    let mut source: Option<String> = None;
    let mut title: Option<String> = None;
    let mut tags: Vec<String> = Vec::new();
    let mut format_value: Option<String> = None;
    let mut encoding: Option<String> = None;

    while let Some(field) = multipart.next_field().await.map_err(rejection)? {
        let name = field.name().unwrap_or_default().to_string();
        match name.as_str() {
            "file" => {
                file_name = field.file_name().map(str::to_string);
                file = Some(field.bytes().await.map_err(rejection)?.to_vec());
            }
            "doc_id" => doc_id = Some(field.text().await.map_err(rejection)?),
            "source" => source = Some(field.text().await.map_err(rejection)?),
            "title" => title = Some(field.text().await.map_err(rejection)?),
            "tags" => {
                let value = field.text().await.map_err(rejection)?;
                tags.extend(
                    value
                        .split(',')
                        .map(str::trim)
                        .filter(|tag| !tag.is_empty())
                        .map(str::to_string),
                );
            }
            "format" => format_value = Some(field.text().await.map_err(rejection)?),
            "encoding" => encoding = Some(field.text().await.map_err(rejection)?),
            _ => {}
        }
    }

    let bytes = file.ok_or_else(|| {
        ApiError(KbError::Validation(
            "multipart field 'file' is required".to_string(),
        ))
    })?;

    // An explicit format is validated before decoding; otherwise the file
    // extension decides, defaulting to plain text.
    let format = match format_value.as_deref() {
        Some(value) => DocumentFormat::parse(value)?,
        None => DocumentFormat::infer_from_filename(file_name.as_deref().unwrap_or_default()),
    };
    let text = decode_bytes(&bytes, encoding.as_deref())?;

    let result = state
        .service
        .ingest_document(
            &collection,
            IngestInput {
                doc_id,
                text,
                format,
                metadata: DocumentMetadata {
                    source,
                    title,
                    tags,
                    extra: serde_json::Map::new(),
                },
            },
        )
        .await?;
    Ok(Json(result))
}

/// `DELETE /v1/collections/{name}/documents?doc_id=...`.
pub async fn delete_document(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    query: Result<Query<DeleteDocumentQuery>, QueryRejection>,
) -> ApiResult<Json<serde_json::Value>> {
    let Query(query) = query.map_err(rejection)?;
    let deleted = state
        .service
        .delete_document(&collection, &query.doc_id)
        .await?;
    Ok(Json(
        json!({ "doc_id": query.doc_id, "deleted_points": deleted }),
    ))
}

/// `POST /v1/collections/{name}/search`.
pub async fn search(
    State(state): State<AppState>,
    Path(collection): Path<String>,
    payload: Result<Json<SearchRequest>, JsonRejection>,
) -> ApiResult<Json<serde_json::Value>> {
    let Json(request) = payload.map_err(rejection)?;
    let results: Vec<SearchHit> = state.service.search(&collection, request).await?;
    Ok(Json(json!({ "results": results })))
}
