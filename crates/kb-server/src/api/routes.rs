//! Router assembly, authentication and common middleware.

use std::{sync::Arc, time::Duration};

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Redirect, Response};
use axum::routing::{delete, get, patch, post};
use axum::Router;
use kb_core::error::KbError;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::{DefaultMakeSpan, DefaultOnResponse, TraceLayer};
use tracing::Level;

use crate::api::{admin, handlers, upload};
use crate::error::{rewrite_status_envelopes, ApiError};
use crate::management::Management;
use crate::state::{AppState, HttpState};

const SETTINGS_WRITE_TIMEOUT: Duration = Duration::from_secs(1800);

/// Builds the complete application router: `/health`, the authenticated `/v1`
/// API and the common middleware stack.
pub fn build_router(state: AppState) -> Router {
    build(HttpState::new(state, None))
}

/// Enables provider management and uses the configured service snapshots.
pub fn build_managed_router(state: AppState, management: Arc<Management>) -> Router {
    build(HttpState::new(state, Some(management)))
}

fn build(state: HttpState) -> Router {
    let max_body_bytes = state.max_body_bytes;
    let timeout = state.request_timeout;
    let upload_timeout = state
        .management()
        .map_or(timeout, |manager| manager.upload_timeout());
    let v1 = collection_routes(timeout, upload_timeout)
        .route_layer(middleware::from_fn_with_state(state.clone(), require_auth));
    let management = collection_routes(timeout, upload_timeout)
        .route(
            "/settings",
            get(admin::get_settings)
                .layer(request_timeout(timeout))
                .merge(
                    patch(admin::patch_settings)
                        .layer(request_timeout(timeout.max(SETTINGS_WRITE_TIMEOUT))),
                ),
        )
        .route(
            "/settings/embedding",
            delete(admin::reset_embedding)
                .layer(request_timeout(timeout.max(SETTINGS_WRITE_TIMEOUT))),
        )
        .route(
            "/settings/embedding/probe",
            post(admin::probe_embedding).layer(request_timeout(timeout)),
        )
        .route(
            "/settings/rerank/probe",
            post(admin::probe_rerank).layer(request_timeout(timeout)),
        )
        .route(
            "/settings/parsing/probe",
            post(admin::probe_parsing).layer(request_timeout(upload_timeout)),
        )
        .route_layer(middleware::from_fn_with_state(
            state.clone(),
            admin::require_admin,
        ))
        .layer(middleware::map_response(admin::prevent_cache));

    let router = Router::new()
        .route(
            "/health",
            get(handlers::health).layer(request_timeout(timeout)),
        )
        .route("/admin", get(|| async { Redirect::permanent("/admin/") }))
        .route("/admin/", get(admin::index))
        .route("/admin/admin.css", get(admin::stylesheet))
        .route("/admin/admin.js", get(admin::javascript))
        .nest("/admin/api", management)
        .nest("/v1", v1)
        .with_state(state);

    with_observability(router, max_body_bytes)
}

/// Applies the middleware shared by every route, inner to outer:
/// body limit, request timeout, status-envelope rewriting and tracing.
///
/// Public so tests can exercise the timeout and body-limit behavior on a
/// minimal router.
pub fn with_common_layers(router: Router, max_body_bytes: usize, timeout: Duration) -> Router {
    with_observability(router.layer(request_timeout(timeout)), max_body_bytes)
}

fn request_timeout(timeout: Duration) -> TimeoutLayer {
    TimeoutLayer::with_status_code(StatusCode::REQUEST_TIMEOUT, timeout)
}

fn with_observability(router: Router, max_body_bytes: usize) -> Router {
    router
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(middleware::map_response(rewrite_status_envelopes))
        // The default TraceLayer levels are DEBUG/TRACE, which the documented
        // `RUST_LOG=info,kb_core=debug` default would not show. Emit the request
        // span (method/path) and the completion event (status/latency) at INFO.
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(DefaultMakeSpan::new().level(Level::INFO))
                .on_response(DefaultOnResponse::new().level(Level::INFO)),
        )
}

/// Routes shared by public API callers and operators, with separate authentication.
fn collection_routes(timeout: Duration, upload_timeout: Duration) -> Router<HttpState> {
    Router::<HttpState>::new()
        .route(
            "/collections",
            get(handlers::list_collections).post(handlers::create_collection),
        )
        .route(
            "/collections/{name}",
            get(handlers::get_collection).delete(handlers::delete_collection),
        )
        .route(
            "/collections/{name}/documents",
            get(admin::list_documents)
                .post(handlers::ingest_document)
                .delete(handlers::delete_document),
        )
        .route(
            "/collections/{name}/documents/chunks",
            get(admin::document_chunks),
        )
        .route("/collections/{name}/search", post(handlers::search))
        .route_layer(request_timeout(timeout))
        .route(
            "/collections/{name}/documents/upload",
            post(upload::upload_document).layer(request_timeout(upload_timeout)),
        )
}

/// Rejects `/v1` requests without the configured bearer token.
async fn require_auth(State(state): State<HttpState>, request: Request, next: Next) -> Response {
    let Some(expected) = state.api_key.as_deref().filter(|key| !key.is_empty()) else {
        return next.run(request).await;
    };

    let provided = request
        .headers()
        .get(header::AUTHORIZATION)
        .and_then(|value| value.to_str().ok());
    let expected_header = format!("Bearer {expected}");
    if provided != Some(expected_header.as_str()) {
        return ApiError(KbError::Unauthorized(
            "missing or invalid bearer token".to_string(),
        ))
        .into_response();
    }

    next.run(request).await
}
