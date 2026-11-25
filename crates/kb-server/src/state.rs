//! Shared state injected into the router, handlers and auth middleware.

use std::sync::Arc;
use std::time::Duration;

use kb_core::models::EmbeddingInfo;
use kb_core::service::KnowledgeBaseService;

/// Everything the HTTP layer needs to handle a request.
#[derive(Clone)]
pub struct AppState {
    /// Knowledge base business logic shared across requests.
    pub service: Arc<KnowledgeBaseService>,
    /// Required bearer token; `None` disables authentication on `/v1`.
    pub api_key: Option<String>,
    /// Maximum accepted request body size, in bytes.
    pub max_body_bytes: usize,
    /// Per-request processing timeout.
    pub request_timeout: Duration,
    /// Config-aware embedding provider details reported by `/health`.
    pub embedding: EmbeddingInfo,
}
