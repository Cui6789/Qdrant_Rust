//! HTTP handlers for the knowledge base REST API.

use std::time::Duration;

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::response::IntoResponse;
use axum::Json;
use kb_core::error::KbError;
use kb_core::models::{
    CollectionInfo, DocumentFormat, HealthStatus, IngestDocumentRequest, IngestResult, SearchHit,
    SearchRequest,
};
use kb_core::service::IngestInput;
use serde::Deserialize;
use serde_json::json;

use crate::error::ApiError;
use crate::state::HttpState;

/// Result type used by every handler.
pub(super) type ApiResult<T> = Result<T, ApiError>;

/// Upper bound for the `/health` probe so a hung Qdrant cannot keep the
/// endpoint waiting for the global `qdrant.timeout_secs`.
const HEALTH_TIMEOUT: Duration = Duration::from_secs(2);

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
pub(super) fn rejection(error: impl IntoResponse + std::fmt::Display) -> ApiError {
    let message = error.to_string();
    if error.into_response().status() == StatusCode::PAYLOAD_TOO_LARGE {
        ApiError(KbError::PayloadTooLarge(message))
    } else {
        ApiError(KbError::Validation(message))
    }
}

/// `GET /health` — Qdrant connectivity and embedding details, no auth.
pub(super) async fn health(State(state): State<HttpState>) -> (StatusCode, Json<HealthStatus>) {
    let (service, embedding) = state.health_snapshot();
    let (status, qdrant) = match tokio::time::timeout(HEALTH_TIMEOUT, service.health()).await {
        Ok(health) => (health.status, health.qdrant),
        Err(_) => {
            tracing::warn!("health check timed out after {HEALTH_TIMEOUT:?}");
            ("degraded".to_string(), "unavailable".to_string())
        }
    };
    let code = if status == "ok" {
        StatusCode::OK
    } else {
        StatusCode::SERVICE_UNAVAILABLE
    };
    (
        code,
        Json(HealthStatus {
            status,
            qdrant,
            embedding,
        }),
    )
}

/// `GET /v1/collections`.
pub(super) async fn list_collections(
    State(state): State<HttpState>,
) -> ApiResult<Json<serde_json::Value>> {
    let collections = state.service().list_collections().await?;
    Ok(Json(json!({ "collections": collections })))
}

/// `POST /v1/collections`.
pub(super) async fn create_collection(
    State(state): State<HttpState>,
    payload: Result<Json<CreateCollectionRequest>, JsonRejection>,
) -> ApiResult<(StatusCode, Json<CollectionInfo>)> {
    let Json(request) = payload.map_err(rejection)?;
    let collection = state.service().create_collection(&request.name).await?;
    Ok((StatusCode::CREATED, Json(collection)))
}

/// `GET /v1/collections/{name}`.
pub(super) async fn get_collection(
    State(state): State<HttpState>,
    Path(name): Path<String>,
) -> ApiResult<Json<CollectionInfo>> {
    Ok(Json(state.service().collection_info(&name).await?))
}

/// `DELETE /v1/collections/{name}`.
pub(super) async fn delete_collection(
    State(state): State<HttpState>,
    Path(name): Path<String>,
) -> ApiResult<StatusCode> {
    state.service().delete_collection(&name).await?;
    Ok(StatusCode::NO_CONTENT)
}

/// `POST /v1/collections/{name}/documents`.
pub(super) async fn ingest_document(
    State(state): State<HttpState>,
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
        .service()
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

/// `DELETE /v1/collections/{name}/documents?doc_id=...`.
pub(super) async fn delete_document(
    State(state): State<HttpState>,
    Path(collection): Path<String>,
    query: Result<Query<DeleteDocumentQuery>, QueryRejection>,
) -> ApiResult<Json<serde_json::Value>> {
    let Query(query) = query.map_err(rejection)?;
    let deleted = state
        .service()
        .delete_document(&collection, &query.doc_id)
        .await?;
    Ok(Json(
        json!({ "doc_id": query.doc_id, "deleted_points": deleted }),
    ))
}

/// `POST /v1/collections/{name}/search`.
pub(super) async fn search(
    State(state): State<HttpState>,
    Path(collection): Path<String>,
    payload: Result<Json<SearchRequest>, JsonRejection>,
) -> ApiResult<Json<serde_json::Value>> {
    let Json(request) = payload.map_err(rejection)?;
    let results: Vec<SearchHit> = state.service().search(&collection, request).await?;
    Ok(Json(json!({ "results": results })))
}
