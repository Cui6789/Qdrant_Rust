//! Shared state injected into the router, handlers and auth middleware.

use std::sync::Arc;
use std::time::Duration;

use kb_core::models::EmbeddingInfo;
use kb_core::service::KnowledgeBaseService;

use crate::management::Management;

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

#[derive(Clone)]
pub(crate) struct HttpState {
    pub api_key: Option<String>,
    pub max_body_bytes: usize,
    pub request_timeout: Duration,
    runtime: Runtime,
}

#[derive(Clone)]
enum Runtime {
    Fixed {
        service: Arc<KnowledgeBaseService>,
        embedding: EmbeddingInfo,
    },
    Managed(Arc<Management>),
}

impl HttpState {
    pub fn new(base: AppState, management: Option<Arc<Management>>) -> Self {
        let runtime = match management {
            Some(management) => Runtime::Managed(management),
            None => Runtime::Fixed {
                service: base.service,
                embedding: base.embedding,
            },
        };
        Self {
            runtime,
            api_key: base.api_key,
            max_body_bytes: base.max_body_bytes,
            request_timeout: base.request_timeout,
        }
    }

    pub fn management(&self) -> Option<&Management> {
        match &self.runtime {
            Runtime::Managed(management) => Some(management),
            Runtime::Fixed { .. } => None,
        }
    }

    pub fn service(&self) -> Arc<KnowledgeBaseService> {
        match &self.runtime {
            Runtime::Managed(management) => management.service(),
            Runtime::Fixed { service, .. } => service.clone(),
        }
    }

    pub fn health_snapshot(&self) -> (Arc<KnowledgeBaseService>, EmbeddingInfo) {
        match &self.runtime {
            Runtime::Managed(management) => {
                let (service, provider) = management.snapshot();
                let embedder = service.embedder();
                let embedding = EmbeddingInfo {
                    provider: provider.as_str().into(),
                    model: embedder.model_id().into(),
                    dimension: embedder.dimension(),
                };
                (service, embedding)
            }
            Runtime::Fixed { service, embedding } => (service.clone(), embedding.clone()),
        }
    }
}
