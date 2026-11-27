//! HTTP error mapping for the dashboard API.

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use kb_core::error::KbError;
use kb_dashboard_types::{ApiErrorBody, ApiErrorDetail};

/// A dashboard API error rendered with the shared envelope.
#[derive(Debug)]
pub struct ApiError {
    pub status: StatusCode,
    pub code: &'static str,
    pub message: String,
}

impl ApiError {
    /// Builds a validation error (`400 VALIDATION_ERROR`).
    pub fn validation(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::BAD_REQUEST,
            code: "VALIDATION_ERROR",
            message: message.into(),
        }
    }

    /// Builds a conflict error with a custom code.
    pub fn conflict(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::CONFLICT,
            code,
            message: message.into(),
        }
    }

    /// Builds a not-found error.
    pub fn not_found(message: impl Into<String>) -> Self {
        Self {
            status: StatusCode::NOT_FOUND,
            code: "NOT_FOUND",
            message: message.into(),
        }
    }
}

impl From<KbError> for ApiError {
    fn from(error: KbError) -> Self {
        let (status, code) = match &error {
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
        };
        Self {
            status,
            code,
            message: error.to_string(),
        }
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            self.status,
            Json(ApiErrorBody {
                error: ApiErrorDetail {
                    code: self.code.to_string(),
                    message: self.message,
                },
            }),
        )
            .into_response()
    }
}
