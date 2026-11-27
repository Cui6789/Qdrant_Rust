use thiserror::Error;

/// Unified error type for the knowledge base core.
#[derive(Debug, Error)]
pub enum KbError {
    #[error("configuration error: {0}")]
    Config(String),
    #[error("validation error: {0}")]
    Validation(String),
    #[error("invalid encoding: {0}")]
    InvalidEncoding(String),
    #[error("unsupported format: {0}")]
    UnsupportedFormat(String),
    #[error("unauthorized: {0}")]
    Unauthorized(String),
    #[error("collection not found: {0}")]
    CollectionNotFound(String),
    #[error("collection already exists: {0}")]
    CollectionExists(String),
    #[error("embedding dimension mismatch: {0}")]
    DimensionMismatch(String),
    #[error("embedding error: {0}")]
    Embedding(String),
    #[error("rerank error: {0}")]
    Rerank(String),
    #[error("qdrant error: {0}")]
    Qdrant(String),
    #[error("payload too large: {0}")]
    PayloadTooLarge(String),
    #[error("parse error: {0}")]
    Parse(String),
    #[error("document parser error: {0}")]
    ParserUnavailable(String),
    #[error("io error: {0}")]
    Io(#[from] std::io::Error),
    #[error("internal error: {0}")]
    Internal(String),
}

/// Convenience result alias used throughout `kb-core`.
pub type Result<T, E = KbError> = std::result::Result<T, E>;
