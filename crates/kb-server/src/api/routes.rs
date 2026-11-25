//! Router assembly, authentication and common middleware.

use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Request, State};
use axum::http::{header, StatusCode};
use axum::middleware::{self, Next};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::Router;
use kb_core::error::KbError;
use tower_http::timeout::TimeoutLayer;
use tower_http::trace::TraceLayer;

use crate::api::handlers;
use crate::error::{rewrite_status_envelopes, ApiError};
use crate::state::AppState;

/// Builds the complete application router: `/health`, the authenticated `/v1`
/// API and the common middleware stack.
pub fn build_router(state: AppState) -> Router {
    let max_body_bytes = state.max_body_bytes;
    let timeout = state.request_timeout;
    let v1 = v1_routes().route_layer(middleware::from_fn_with_state(state.clone(), require_auth));

    let router = Router::new()
        .route("/health", get(handlers::health))
        .nest("/v1", v1)
        .with_state(state);

    with_common_layers(router, max_body_bytes, timeout)
}

/// Applies the middleware shared by every route, inner to outer:
/// body limit, request timeout, status-envelope rewriting and tracing.
///
/// Public so tests can exercise the timeout and body-limit behavior on a
/// minimal router.
pub fn with_common_layers(router: Router, max_body_bytes: usize, timeout: Duration) -> Router {
    router
        .layer(DefaultBodyLimit::max(max_body_bytes))
        .layer(TimeoutLayer::with_status_code(
            StatusCode::REQUEST_TIMEOUT,
            timeout,
        ))
        .layer(middleware::map_response(rewrite_status_envelopes))
        .layer(TraceLayer::new_for_http())
}

/// Routes under `/v1`; authentication is applied by the caller.
fn v1_routes() -> Router<AppState> {
    Router::<AppState>::new()
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
            post(handlers::ingest_document).delete(handlers::delete_document),
        )
        .route(
            "/collections/{name}/documents/upload",
            post(handlers::upload_document),
        )
        .route("/collections/{name}/search", post(handlers::search))
}

/// Rejects `/v1` requests without the configured bearer token.
async fn require_auth(State(state): State<AppState>, request: Request, next: Next) -> Response {
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
