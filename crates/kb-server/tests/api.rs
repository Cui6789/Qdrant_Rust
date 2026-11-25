//! HTTP integration tests for the knowledge base server.
//!
//! Tests 1-7 exercise auth, validation and the common middleware without a
//! Qdrant instance. Tests 8-10 need a live Qdrant and are gated on
//! `KB_TEST_QDRANT_URL`; they print a skip line and return when it is unset.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use axum::body::Body;
use axum::http::{header, Method, Request, StatusCode};
use axum::Router;
use http_body_util::BodyExt;
use kb_core::config::{ApiEmbeddingConfig, IngestConfig, QdrantConfig};
use kb_core::embedding::api::ApiEmbedding;
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::embedding::EmbeddingProvider;
use kb_core::models::EmbeddingInfo;
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;
use kb_server::api::{build_router, with_common_layers};
use kb_server::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Bearer token used by the authenticated test states.
const API_KEY: &str = "secret";
/// Qdrant address that is guaranteed to refuse connections.
const UNREACHABLE_QDRANT: &str = "http://127.0.0.1:1";

fn qdrant_url() -> Option<String> {
    match std::env::var("KB_TEST_QDRANT_URL") {
        Ok(value) if !value.trim().is_empty() => Some(value),
        _ => {
            eprintln!("skipping: KB_TEST_QDRANT_URL not set");
            None
        }
    }
}

fn unique_name(prefix: &str) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let counter = COUNTER.fetch_add(1, Ordering::SeqCst);
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("system clock is after the epoch")
        .as_nanos();
    format!("t7{prefix}{nanos:x}{counter}")
}

fn qdrant_config(url: &str, timeout_secs: u64) -> QdrantConfig {
    QdrantConfig {
        url: url.to_string(),
        api_key: String::new(),
        timeout_secs,
        startup_retry_secs: 30,
    }
}

fn ingest_config() -> IngestConfig {
    IngestConfig {
        chunk_size: 400,
        chunk_overlap: 40,
        min_chunk_size: 1,
        upsert_batch_size: 8,
    }
}

fn deterministic_embedder() -> Arc<dyn EmbeddingProvider> {
    Arc::new(DeterministicEmbedding::new(8))
}

fn service_for(
    url: &str,
    timeout_secs: u64,
    embedder: Arc<dyn EmbeddingProvider>,
) -> Arc<KnowledgeBaseService> {
    let store =
        Arc::new(QdrantStore::new(&qdrant_config(url, timeout_secs)).expect("store builds"));
    Arc::new(KnowledgeBaseService::new(store, embedder, ingest_config()))
}

fn local_embedding_info() -> EmbeddingInfo {
    EmbeddingInfo {
        provider: "local".to_string(),
        model: "deterministic-mock".to_string(),
        dimension: 8,
    }
}

fn api_embedding_info() -> EmbeddingInfo {
    EmbeddingInfo {
        provider: "api".to_string(),
        model: "test-model".to_string(),
        dimension: 8,
    }
}

fn app_state(
    service: Arc<KnowledgeBaseService>,
    api_key: Option<&str>,
    embedding: EmbeddingInfo,
    request_timeout_secs: u64,
) -> AppState {
    AppState {
        service,
        api_key: api_key.map(str::to_string),
        max_body_bytes: 8 * 1024 * 1024,
        request_timeout: Duration::from_secs(request_timeout_secs),
        embedding,
    }
}

/// Router whose data path cannot reach Qdrant; used for validation tests.
fn offline_app() -> Router {
    let service = service_for(UNREACHABLE_QDRANT, 1, deterministic_embedder());
    build_router(app_state(
        service,
        Some(API_KEY),
        local_embedding_info(),
        10,
    ))
}

fn request(
    method: Method,
    uri: &str,
    token: Option<&str>,
    content_type: Option<&str>,
    body: Body,
) -> Request<Body> {
    let mut builder = Request::builder().method(method).uri(uri);
    if let Some(token) = token {
        builder = builder.header(header::AUTHORIZATION, format!("Bearer {token}"));
    }
    if let Some(content_type) = content_type {
        builder = builder.header(header::CONTENT_TYPE, content_type);
    }
    builder.body(body).expect("request builds")
}

fn json_request(method: Method, uri: &str, token: Option<&str>, body: &Value) -> Request<Body> {
    request(
        method,
        uri,
        token,
        Some("application/json"),
        Body::from(body.to_string()),
    )
}

async fn send(app: &Router, request: Request<Body>) -> (StatusCode, Vec<u8>) {
    let response = app
        .clone()
        .oneshot(request)
        .await
        .expect("router handles the request");
    let status = response.status();
    let bytes = response
        .into_body()
        .collect()
        .await
        .expect("response body collects")
        .to_bytes();
    (status, bytes.to_vec())
}

async fn send_json(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
    let (status, bytes) = send(app, request).await;
    let value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    (status, value)
}

#[tokio::test]
async fn auth_rejects_missing_or_wrong_token() {
    let app = offline_app();

    for token in [None, Some("wrong")] {
        let (status, body) = send_json(
            &app,
            json_request(
                Method::POST,
                "/v1/collections",
                token,
                &json!({"name": "docs"}),
            ),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
        assert_eq!(body["error"]["code"], "UNAUTHORIZED");
        assert!(body["error"]["message"].is_string());
    }
}

#[tokio::test]
async fn auth_accepts_valid_token_and_validation_runs() {
    let app = offline_app();

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            "/v1/collections",
            Some(API_KEY),
            &json!({"name": "bad name!"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn malformed_json_returns_validation_envelope() {
    let app = offline_app();

    let (status, body) = send_json(
        &app,
        request(
            Method::POST,
            "/v1/collections",
            Some(API_KEY),
            Some("application/json"),
            Body::from("{"),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["error"]["code"], "VALIDATION_ERROR");
}

#[tokio::test]
async fn health_is_unauthenticated_and_reports_degraded() {
    let service = service_for(UNREACHABLE_QDRANT, 1, deterministic_embedder());
    let app = build_router(app_state(
        service,
        Some(API_KEY),
        local_embedding_info(),
        10,
    ));

    let (status, body) = send_json(
        &app,
        request(Method::GET, "/health", None, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::SERVICE_UNAVAILABLE);
    assert_eq!(body["status"], "degraded");
    assert_eq!(body["qdrant"], "unavailable");
    assert_eq!(body["embedding"]["provider"], "local");
}

#[tokio::test]
async fn timeout_returns_408_envelope() {
    async fn slow() -> &'static str {
        tokio::time::sleep(Duration::from_millis(500)).await;
        "done"
    }

    let router = Router::new().route("/slow", axum::routing::get(slow));
    let app = with_common_layers(router, 1024, Duration::from_millis(25));

    let (status, body) = send_json(
        &app,
        request(Method::GET, "/slow", None, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::REQUEST_TIMEOUT);
    assert_eq!(body["error"]["code"], "REQUEST_TIMEOUT");
}

#[tokio::test]
async fn body_limit_returns_413_envelope() {
    async fn accept(_body: axum::body::Bytes) -> StatusCode {
        StatusCode::OK
    }

    let router = Router::new().route("/echo", axum::routing::post(accept));
    let app = with_common_layers(router, 16, Duration::from_secs(5));

    let (status, body) = send_json(
        &app,
        request(
            Method::POST,
            "/echo",
            None,
            Some("application/octet-stream"),
            Body::from(vec![b'a'; 1024]),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::PAYLOAD_TOO_LARGE);
    assert_eq!(body["error"]["code"], "PAYLOAD_TOO_LARGE");
}

#[tokio::test]
async fn unknown_format_returns_415() {
    let app = offline_app();

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            "/v1/collections/docs/documents",
            Some(API_KEY),
            &json!({"text": "hi", "format": "pdf"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::UNSUPPORTED_MEDIA_TYPE);
    assert_eq!(body["error"]["code"], "UNSUPPORTED_FORMAT");
}

#[tokio::test]
async fn full_lifecycle_over_http() {
    let Some(url) = qdrant_url() else { return };
    let service = service_for(&url, 10, deterministic_embedder());
    let app = build_router(app_state(
        service,
        Some(API_KEY),
        local_embedding_info(),
        60,
    ));
    let name = unique_name("life");
    let collection_uri = format!("/v1/collections/{name}");
    let documents_uri = format!("/v1/collections/{name}/documents");
    let search_uri = format!("/v1/collections/{name}/search");
    let upload_uri = format!("/v1/collections/{name}/documents/upload");

    // Create the collection.
    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            "/v1/collections",
            Some(API_KEY),
            &json!({"name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["name"], name);
    assert_eq!(body["points_count"], 0);
    assert_eq!(body["vector_size"], 8);

    // Ingest a markdown document with tags.
    let document = json!({
        "doc_id": "handbook",
        "text": "# Handbook\n\nAnnual leave is requested in the HR system.",
        "format": "markdown",
        "metadata": {
            "source": "handbook.md",
            "title": "Employee Handbook",
            "tags": ["hr", "policy"]
        }
    });
    let (status, body) = send_json(
        &app,
        json_request(Method::POST, &documents_uri, Some(API_KEY), &document),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["doc_id"], "handbook");
    assert_eq!(body["skipped"], false);
    assert!(body["chunks"].as_u64().expect("chunks is a number") >= 1);

    // Search finds the ingested text.
    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &search_uri,
            Some(API_KEY),
            &json!({"query": "annual leave", "top_k": 5}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let results = body["results"].as_array().expect("results array");
    assert!(!results.is_empty(), "expected at least one search hit");
    assert!(results.iter().any(|hit| hit["text"]
        .as_str()
        .unwrap_or_default()
        .contains("Annual leave")));

    // Re-ingesting the same document is skipped.
    let (status, body) = send_json(
        &app,
        json_request(Method::POST, &documents_uri, Some(API_KEY), &document),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["skipped"], true);

    // Upload a text file through multipart.
    let boundary = "X-KB-TEST-BOUNDARY";
    let multipart_body = [
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"doc_id\"\r\n\r\nuploaded-doc\r\n"),
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"title\"\r\n\r\nUploaded Notes\r\n"),
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"tags\"\r\n\r\nhr,travel\r\n"),
        format!("--{boundary}\r\nContent-Disposition: form-data; name=\"file\"; filename=\"notes.txt\"\r\nContent-Type: text/plain\r\n\r\nUploaded policy covers travel expenses.\r\n"),
        format!("--{boundary}--\r\n"),
    ]
    .concat();
    let (status, body) = send_json(
        &app,
        request(
            Method::POST,
            &upload_uri,
            Some(API_KEY),
            Some(&format!("multipart/form-data; boundary={boundary}")),
            Body::from(multipart_body),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["doc_id"], "uploaded-doc");
    assert!(body["chunks"].as_u64().expect("chunks is a number") >= 1);

    // Delete the uploaded document by query parameter.
    let (status, body) = send_json(
        &app,
        request(
            Method::DELETE,
            &format!("{documents_uri}?doc_id=uploaded-doc"),
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["doc_id"], "uploaded-doc");
    assert!(body["deleted_points"].as_u64().expect("count is a number") >= 1);

    // List collections.
    let (status, body) = send_json(
        &app,
        request(
            Method::GET,
            "/v1/collections",
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let names: Vec<&str> = body["collections"]
        .as_array()
        .expect("collections array")
        .iter()
        .filter_map(|collection| collection["name"].as_str())
        .collect();
    assert!(
        names.contains(&name.as_str()),
        "missing {name} in {names:?}"
    );

    // Read one collection.
    let (status, body) = send_json(
        &app,
        request(
            Method::GET,
            &collection_uri,
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["name"], name);

    // Delete the collection.
    let (status, bytes) = send(
        &app,
        request(
            Method::DELETE,
            &collection_uri,
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
    assert!(bytes.is_empty(), "204 responses have no body");
}

#[tokio::test]
async fn unicode_doc_id_delete_over_http() {
    let Some(url) = qdrant_url() else { return };
    let service = service_for(&url, 10, deterministic_embedder());
    let app = build_router(app_state(
        service,
        Some(API_KEY),
        local_embedding_info(),
        60,
    ));
    let name = unique_name("unicode");
    let collection_uri = format!("/v1/collections/{name}");
    let documents_uri = format!("/v1/collections/{name}/documents");

    let (status, _) = send_json(
        &app,
        json_request(
            Method::POST,
            "/v1/collections",
            Some(API_KEY),
            &json!({"name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);

    let doc_id = "手册/2026 版";
    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &documents_uri,
            Some(API_KEY),
            &json!({"doc_id": doc_id, "text": "unicode doc id body"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["doc_id"], doc_id);

    // URL-encoded query value: 手册/2026 版
    let encoded = "%E6%89%8B%E5%86%8C%2F2026%20%E7%89%88";
    let (status, body) = send_json(
        &app,
        request(
            Method::DELETE,
            &format!("{documents_uri}?doc_id={encoded}"),
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["doc_id"], doc_id);
    assert!(body["deleted_points"].as_u64().expect("count is a number") > 0);

    let (status, _) = send(
        &app,
        request(
            Method::DELETE,
            &collection_uri,
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn api_provider_end_to_end() {
    let Some(url) = qdrant_url() else { return };
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).expect("request body is JSON");
            let inputs = body["input"].as_array().expect("input is an array");
            let data: Vec<Value> = inputs
                .iter()
                .enumerate()
                .map(|(index, _)| {
                    json!({
                        "index": index,
                        "embedding": [0.1_f32, 0.2, 0.3, 0.4, 0.5, 0.6, 0.7, 0.8]
                    })
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .mount(&server)
        .await;

    let config = ApiEmbeddingConfig {
        base_url: format!("{}/v1", server.uri()),
        api_key: String::new(),
        model: "test-model".to_string(),
        dimensions: 8,
        timeout_secs: 5,
        max_retries: 0,
    };
    let embedder = Arc::new(
        ApiEmbedding::new(&config, 8, 1)
            .await
            .expect("api embedding builds"),
    );
    assert_eq!(embedder.dimension(), 8);

    let service = service_for(&url, 10, embedder);
    let app = build_router(app_state(service, Some(API_KEY), api_embedding_info(), 60));
    let name = unique_name("api");
    let collection_uri = format!("/v1/collections/{name}");
    let documents_uri = format!("/v1/collections/{name}/documents");
    let search_uri = format!("/v1/collections/{name}/search");

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            "/v1/collections",
            Some(API_KEY),
            &json!({"name": name}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED);
    assert_eq!(body["vector_size"], 8);

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &documents_uri,
            Some(API_KEY),
            &json!({
                "doc_id": "api-doc",
                "text": "Network embedding end to end text."
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["embedding_model"], "test-model");

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &search_uri,
            Some(API_KEY),
            &json!({"query": "embedding", "top_k": 3}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert!(!body["results"]
        .as_array()
        .expect("results array")
        .is_empty());

    let (status, _) = send(
        &app,
        request(
            Method::DELETE,
            &collection_uri,
            Some(API_KEY),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}
