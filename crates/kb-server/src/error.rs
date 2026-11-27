//! HTTP error mapping.
//!
//! [`ApiError`] converts a [`KbError`] into the stable error envelope used by
//! every endpoint: `{"error":{"code":"...","message":"..."}}`. The common
//! middleware also rewrites the empty 408/413 responses produced by
//! `tower-http` and `DefaultBodyLimit` into the same envelope.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kb_core::error::KbError;
use serde_json::json;

/// A [`KbError`] rendered as an HTTP error response.
#[derive(Debug)]
pub struct ApiError(pub KbError);

impl ApiError {
    /// Selects the HTTP status and stable machine-readable code for an error.
    fn status_and_code(&self) -> (StatusCode, &'static str) {
        match &self.0 {
            KbError::Config(_) | KbError::Internal(_) | KbError::Io(_) => {
                (StatusCode::INTERNAL_SERVER_ERROR, "INTERNAL")
            }
            KbError::Validation(_) | KbError::Parse(_) => {
                (StatusCode::BAD_REQUEST, "VALIDATION_ERROR")
            }
            KbError::InvalidEncoding(_) => (StatusCode::BAD_REQUEST, "INVALID_ENCODING"),
            KbError::UnsupportedFormat(_) => {
                (StatusCode::UNSUPPORTED_MEDIA_TYPE, "UNSUPPORTED_FORMAT")
            }
            KbError::Unauthorized(_) => (StatusCode::UNAUTHORIZED, "UNAUTHORIZED"),
            KbError::CollectionNotFound(_) => (StatusCode::NOT_FOUND, "COLLECTION_NOT_FOUND"),
            KbError::CollectionExists(_) => (StatusCode::CONFLICT, "COLLECTION_EXISTS"),
            KbError::DimensionMismatch(_) => (StatusCode::CONFLICT, "EMBEDDING_DIM_MISMATCH"),
            KbError::Embedding(_) => (StatusCode::BAD_GATEWAY, "EMBEDDING_ERROR"),
            KbError::Rerank(_) => (StatusCode::BAD_GATEWAY, "RERANK_ERROR"),
            KbError::ParserUnavailable(_) => (StatusCode::BAD_GATEWAY, "PARSER_ERROR"),
            KbError::Qdrant(_) => (StatusCode::BAD_GATEWAY, "QDRANT_ERROR"),
            KbError::PayloadTooLarge(_) => (StatusCode::PAYLOAD_TOO_LARGE, "PAYLOAD_TOO_LARGE"),
        }
    }
}

impl From<KbError> for ApiError {
    fn from(error: KbError) -> Self {
        Self(error)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, code) = self.status_and_code();
        let message = self.0.to_string();
        (
            status,
            Json(json!({"error": {"code": code, "message": message}})),
        )
            .into_response()
    }
}

/// Replaces the empty 408 and 413 responses emitted by middleware layers with
/// the standard error envelope, leaving every other response untouched.
pub async fn rewrite_status_envelopes(response: Response) -> Response {
    let (code, message) = match response.status() {
        StatusCode::REQUEST_TIMEOUT => ("REQUEST_TIMEOUT", "request timed out"),
        StatusCode::PAYLOAD_TOO_LARGE => ("PAYLOAD_TOO_LARGE", "request body is too large"),
        _ => return response,
    };
    (
        response.status(),
        Json(json!({"error": {"code": code, "message": message}})),
    )
        .into_response()
}
