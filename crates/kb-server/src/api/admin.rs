//! Administration routes and the embedded browser client.

use axum::extract::rejection::{JsonRejection, QueryRejection};
use axum::extract::{Path, Query, Request, State};
use axum::http::{header, HeaderValue, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kb_core::error::KbError;
use kb_core::models::{DocumentChunk, DocumentSummary, Page};
use serde::Deserialize;
use serde_json::json;

use super::handlers::{rejection, ApiResult};
use crate::error::ApiError;
use crate::management::{EmbeddingProbe, ParsingProbe, RerankProbe, SettingsPatch, SettingsView};
use crate::state::HttpState;

const CONTENT_POLICY: &str = "default-src 'none'; script-src 'self'; style-src 'self'; img-src 'self' data:; connect-src 'self'; font-src 'self'; base-uri 'none'; frame-ancestors 'none'; form-action 'none'";

pub(super) async fn index() -> Response {
    asset(
        "text/html; charset=utf-8",
        include_str!("../../admin/index.html"),
    )
}

pub(super) async fn stylesheet() -> Response {
    asset(
        "text/css; charset=utf-8",
        include_str!("../../admin/admin.css"),
    )
}

pub(super) async fn javascript() -> Response {
    asset(
        "text/javascript; charset=utf-8",
        include_str!("../../admin/admin.js"),
    )
}

fn asset(content_type: &'static str, content: &'static str) -> Response {
    (
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_SECURITY_POLICY, CONTENT_POLICY),
            (header::CACHE_CONTROL, "no-cache"),
            (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
        ],
        content,
    )
        .into_response()
}

pub(super) async fn prevent_cache(mut response: Response) -> Response {
    response
        .headers_mut()
        .insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    response
}

pub(super) async fn require_admin(
    State(state): State<HttpState>,
    request: Request,
    next: Next,
) -> Response {
    let Some(management) = state.management().filter(|manager| manager.enabled()) else {
        return (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(json!({"error": {
                "code": "ADMIN_DISABLED",
                "message": "set KB_ADMIN__ACCESS_KEY to enable the management API"
            }})),
        )
            .into_response();
    };

    let token = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.strip_prefix("Bearer "));
    if !token.is_some_and(|value| management.accepts(value)) {
        return ApiError(KbError::Unauthorized(
            "invalid management access key".into(),
        ))
        .into_response();
    }
    next.run(request).await
}

pub(super) async fn get_settings(State(state): State<HttpState>) -> ApiResult<Json<SettingsView>> {
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .view(),
    ))
}

pub(super) async fn patch_settings(
    State(state): State<HttpState>,
    payload: Result<Json<SettingsPatch>, JsonRejection>,
) -> ApiResult<Json<SettingsView>> {
    let Json(patch) = payload.map_err(|error| {
        if error.status() == StatusCode::PAYLOAD_TOO_LARGE {
            return rejection(error);
        }
        ApiError(KbError::Validation("invalid settings payload".into()))
    })?;
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .apply(patch)
            .await?,
    ))
}

pub(super) async fn probe_embedding(
    State(state): State<HttpState>,
) -> ApiResult<Json<EmbeddingProbe>> {
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .probe_embedding()
            .await?,
    ))
}

pub(super) async fn reset_embedding(
    State(state): State<HttpState>,
) -> ApiResult<Json<SettingsView>> {
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .reset_embedding()
            .await?,
    ))
}

pub(super) async fn probe_rerank(State(state): State<HttpState>) -> ApiResult<Json<RerankProbe>> {
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .probe_rerank()
            .await?,
    ))
}

pub(super) async fn probe_parsing(State(state): State<HttpState>) -> ApiResult<Json<ParsingProbe>> {
    Ok(Json(
        state
            .management()
            .expect("authenticated management route")
            .probe_parsing()
            .await?,
    ))
}

#[derive(Deserialize)]
pub(super) struct PageQuery {
    #[serde(default = "default_limit")]
    limit: u32,
    cursor: Option<String>,
}

#[derive(Deserialize)]
pub(super) struct ChunkQuery {
    doc_id: String,
    #[serde(default = "default_limit")]
    limit: u32,
    cursor: Option<String>,
}

fn default_limit() -> u32 {
    25
}

pub(super) async fn list_documents(
    State(state): State<HttpState>,
    Path(collection): Path<String>,
    query: Result<Query<PageQuery>, QueryRejection>,
) -> ApiResult<Json<Page<DocumentSummary>>> {
    let Query(query) = query.map_err(rejection)?;
    Ok(Json(
        state
            .service()
            .list_documents(&collection, query.limit, query.cursor.as_deref())
            .await?,
    ))
}

pub(super) async fn document_chunks(
    State(state): State<HttpState>,
    Path(collection): Path<String>,
    query: Result<Query<ChunkQuery>, QueryRejection>,
) -> ApiResult<Json<Page<DocumentChunk>>> {
    let Query(query) = query.map_err(rejection)?;
    Ok(Json(
        state
            .service()
            .document_chunks(
                &collection,
                &query.doc_id,
                query.limit,
                query.cursor.as_deref(),
            )
            .await?,
    ))
}
