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
use kb_core::config::{ApiEmbeddingConfig, AppConfig, IngestConfig, ProviderKind, QdrantConfig};
use kb_core::embedding::api::ApiEmbedding;
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::embedding::EmbeddingProvider;
use kb_core::models::EmbeddingInfo;
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;
use kb_server::api::{build_managed_router, build_router, with_common_layers};
use kb_server::management::Management;
use kb_server::state::AppState;
use serde_json::{json, Value};
use tower::ServiceExt;
use wiremock::matchers::{method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

/// Bearer token used by the authenticated test states.
const API_KEY: &str = "secret";
/// Qdrant address that is guaranteed to refuse connections.
const UNREACHABLE_QDRANT: &str = "http://127.0.0.1:1";

#[tokio::test]
async fn management_upload_and_rerank_share_live_configuration() {
    let Some(url) = qdrant_url() else { return };
    let upstream = MockServer::start().await;
    let embedding_original = "start-embedding-test-key";
    let embedding_rotated = "rotated-embedding-test-key";
    let rerank_key = "rerank-test-key";
    let admin_key = "management-test-key";
    let markdown = "# 差旅费用\n\n经批准的交通和住宿费用可在报销系统中提交。";
    let uri = upstream.uri();
    let settings_dir = tempfile::tempdir().unwrap();

    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {embedding_original}").as_str(),
        ))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"index": 0, "embedding": [1.0, 0.0]}]
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {embedding_rotated}").as_str(),
        ))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert!(
                body.get("dimensions").is_none(),
                "inferred dimensions stay off the wire"
            );
            let data: Vec<_> = body["input"]
                .as_array()
                .unwrap()
                .iter()
                .enumerate()
                .map(|(index, _)| json!({"index": index, "embedding": [1.0, 0.0]}))
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({"data": data}))
        })
        .expect(4..)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/file_parse"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "backend": "pipeline",
            "version": "3",
            "results": {"fixture": {"md_content": markdown}}
        })))
        .expect(1)
        .mount(&upstream)
        .await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .and(wiremock::matchers::header(
            "Authorization",
            format!("Bearer {rerank_key}").as_str(),
        ))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            let docs = body["documents"].as_array().unwrap();
            assert_eq!(docs.len(), 2, "recall two candidates for top_k=1");
            let results: Vec<_> = docs
                .iter()
                .enumerate()
                .map(|(index, doc)| {
                    let score = if doc.as_str().unwrap().contains("报销系统") {
                        0.98
                    } else {
                        0.2
                    };
                    json!({"index": index, "relevance_score": score})
                })
                .collect();
            ResponseTemplate::new(200).set_body_json(json!({ "results": results }))
        })
        .expect(1)
        .mount(&upstream)
        .await;

    let mut config = AppConfig {
        qdrant: qdrant_config(&url, 45),
        ingest: ingest_config(),
        ..AppConfig::default()
    };
    config.embedding.provider = ProviderKind::Api;
    config.embedding.api.base_url = format!("{uri}/v1");
    config.embedding.api.api_key = embedding_original.to_string();
    config.embedding.api.model = "test-model".to_string();
    config.embedding.api.dimensions = 0;
    config.embedding.api.max_retries = 0;
    config.admin.access_key = admin_key.to_string();
    config.admin.settings_path = settings_dir.path().join("runtime.json");
    config.parsing.mineru.local_url = format!("{uri}/file_parse");
    let store = Arc::new(QdrantStore::new(&config.qdrant).unwrap());
    let manager = Arc::new(
        Management::new(config.clone(), store.clone())
            .await
            .unwrap(),
    );
    let app = build_managed_router(
        app_state(manager.service(), Some(API_KEY), api_embedding_info(), 60),
        manager,
    );

    let (status, _) = send(
        &app,
        request(Method::GET, "/admin/", None, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    let settings = "/admin/api/settings";
    let (status, _) = send_json(
        &app,
        json_request(Method::GET, settings, Some(API_KEY), &json!(null)),
    )
    .await;
    assert_eq!(status, StatusCode::UNAUTHORIZED);
    let (status, body) = send_json(
        &app,
        json_request(
            Method::PATCH,
            settings,
            Some(admin_key),
            &json!({
                "embedding_api_key": embedding_rotated,
                "rerank_provider": "api",
                "rerank_api_endpoint": format!("{uri}/v1/rerank"),
                "rerank_api_model": "cross-encoder",
                "rerank_api_key": rerank_key,
                "rerank_candidate_limit": 2,
                "parsing_provider": "mineru_local"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["embedding_api_key_set"], true);
    assert_eq!(body["rerank_api_key_set"], true);
    assert!(body.get("embedding_api_key").is_none());
    assert!(body.get("rerank_api_key").is_none());

    let (status, _) = send_json(
        &app,
        json_request(
            Method::PATCH,
            settings,
            Some(admin_key),
            &json!({
                "parsing_provider": "mineru_cloud"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let restarted = Management::new(config, store).await.unwrap();
    assert_eq!(restarted.service().embedder().dimension(), 2);

    let name = unique_name("management");
    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            "/admin/api/collections",
            Some(admin_key),
            &json!({ "name": name }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::CREATED, "{body}");

    let collection = format!("/admin/api/collections/{name}");
    let documents = format!("{collection}/documents");
    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &documents,
            Some(admin_key),
            &json!({
                "doc_id": "native",
                "text": "请先在考勤系统登记休假。"
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");

    let payload = concat!(
        "--fixture\r\nContent-Disposition: form-data; name=\"doc_id\"\r\n\r\nexpenses\r\n",
        "--fixture\r\nContent-Disposition: form-data; name=\"file\"; filename=\"fixture.pdf\"\r\n",
        "Content-Type: application/pdf\r\n\r\n%PDF-1.4 document fixture\r\n--fixture--\r\n",
    );
    let (status, body) = send_json(
        &app,
        request(
            Method::POST,
            &format!("{documents}/upload"),
            Some(admin_key),
            Some("multipart/form-data; boundary=fixture"),
            Body::from(payload),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["doc_id"], "expenses");

    let (status, page) = send_json(
        &app,
        request(
            Method::GET,
            &format!("{documents}?limit=1"),
            Some(admin_key),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(page["items"].as_array().unwrap().len(), 1);
    let cursor = page["next_cursor"].as_str().expect("second page");
    let (status, second) = send_json(
        &app,
        request(
            Method::GET,
            &format!("{documents}?limit=1&cursor={cursor}"),
            Some(admin_key),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_ne!(page["items"][0]["doc_id"], second["items"][0]["doc_id"]);

    let (status, body) = send_json(
        &app,
        request(
            Method::GET,
            &format!("{documents}/chunks?doc_id=expenses&limit=1"),
            Some(admin_key),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert!(body["items"][0]["text"]
        .as_str()
        .unwrap()
        .contains("报销系统"));

    let (status, body) = send_json(
        &app,
        json_request(
            Method::POST,
            &format!("/v1/collections/{name}/search"),
            Some(API_KEY),
            &json!({"query": "如何报销差旅费", "top_k": 1}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["results"][0]["doc_id"], "expenses");
    assert!(body["results"][0]["rerank_score"].as_f64().unwrap() > 0.9);
    assert!(body["results"][0]["score"].is_number());

    let (status, body) = send_json(
        &app,
        request(
            Method::DELETE,
            &format!("{documents}?doc_id=expenses"),
            Some(admin_key),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deleted_points"], 1);
    let (status, _) = send(
        &app,
        request(
            Method::DELETE,
            &collection,
            Some(admin_key),
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT);
}

#[tokio::test]
async fn settings_probes_exercise_the_saved_services_without_qdrant() {
    let settings_dir = tempfile::tempdir().unwrap();
    let mock = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/rerank"))
        .and(wiremock::matchers::header(
            "Authorization",
            "Bearer test-only-rerank-key",
        ))
        .respond_with(|request: &wiremock::Request| {
            let body: Value = serde_json::from_slice(&request.body).unwrap();
            assert_eq!(body["model"], "probe-reranker");
            assert_eq!(body["documents"].as_array().unwrap().len(), 2);
            ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"index": 0, "relevance_score": 0.8},
                    {"index": 1, "relevance_score": 0.1}
                ]
            }))
        })
        .expect(1)
        .mount(&mock)
        .await;
    Mock::given(method("POST"))
        .and(path("/file_parse"))
        .respond_with(|request: &wiremock::Request| {
            assert!(request
                .body
                .windows(10)
                .any(|window| window == b"sample.pdf"));
            assert!(request.body.windows(8).any(|window| window == b"%PDF-1.4"));
            ResponseTemplate::new(200).set_body_json(json!({
                "results": {"sample": {"md_content": "# Parsed\n\nSample fixture."}}
            }))
        })
        .expect(1)
        .mount(&mock)
        .await;

    let mut config = AppConfig {
        qdrant: qdrant_config(UNREACHABLE_QDRANT, 2),
        ..AppConfig::default()
    };
    config.embedding.provider = ProviderKind::Mock;
    config.admin.access_key = "probe-test-admin".into();
    config.admin.settings_path = settings_dir.path().join("runtime.json");
    let store = Arc::new(QdrantStore::new(&config.qdrant).unwrap());
    let admin_token = Some(config.admin.access_key.as_str());
    let manager = Arc::new(Management::new(config.clone(), store).await.unwrap());
    let app = build_managed_router(
        app_state(manager.service(), Some(API_KEY), local_embedding_info(), 60),
        manager,
    );
    let rerank = "/admin/api/settings/rerank/probe";
    let parsing = "/admin/api/settings/parsing/probe";
    for route in [rerank, parsing] {
        let (status, _) = send(
            &app,
            request(Method::POST, route, Some(API_KEY), None, Body::empty()),
        )
        .await;
        assert_eq!(status, StatusCode::UNAUTHORIZED);
    }
    let (status, _) = send(
        &app,
        request(Method::POST, rerank, admin_token, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    let (status, native) = send_json(
        &app,
        request(Method::POST, parsing, admin_token, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{native}");
    assert_eq!(native["provider"], "native");
    assert_eq!(native["fixture"], "sample.md");
    assert!(!config.admin.settings_path.exists());

    let (status, settings) = send_json(
        &app,
        json_request(
            Method::PATCH,
            "/admin/api/settings",
            admin_token,
            &json!({
                "rerank_provider": "api",
                "rerank_api_model": "probe-reranker",
                "rerank_api_endpoint": format!("{}/v1/rerank", mock.uri()),
                "rerank_api_key": "test-only-rerank-key",
                "parsing_provider": "mineru_local",
                "mineru_local_url": format!("{}/file_parse", mock.uri()),
            }),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{settings}");
    let before = std::fs::read(&config.admin.settings_path).unwrap();
    let (status, ranked) = send_json(
        &app,
        request(Method::POST, rerank, admin_token, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{ranked}");
    assert_eq!(ranked["provider"], "api");
    assert_eq!(ranked["model"], "probe-reranker");
    assert_eq!(ranked["scored"], 2);

    let (status, parsed) = send_json(
        &app,
        request(Method::POST, parsing, admin_token, None, Body::empty()),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{parsed}");
    assert_eq!(parsed["provider"], "mineru_local");
    assert_eq!(parsed["fixture"], "sample.pdf");
    assert!(parsed["characters"].as_u64().unwrap() > 0);
    assert_eq!(std::fs::read(&config.admin.settings_path).unwrap(), before);
}

#[cfg(feature = "local-embedding")]
#[tokio::test]
async fn custom_local_dimensions_validate_persist_and_reset() {
    let dir = tempfile::tempdir().unwrap();
    let mut config = AppConfig {
        qdrant: qdrant_config(UNREACHABLE_QDRANT, 2),
        ..AppConfig::default()
    };
    // Validate native dimensions using model metadata, without downloading weights.
    config.embedding.local.warmup = false;
    config.admin.access_key = "local-dimension-admin".into();
    config.admin.settings_path = dir.path().join("runtime.json");
    let store = Arc::new(QdrantStore::new(&config.qdrant).unwrap());
    let manager = Arc::new(
        Management::new(config.clone(), store.clone())
            .await
            .unwrap(),
    );
    let app = build_managed_router(
        app_state(manager.service(), Some(API_KEY), local_embedding_info(), 60),
        manager.clone(),
    );
    let token = Some(config.admin.access_key.as_str());
    let settings = "/admin/api/settings";
    let (status, saved) = send_json(
        &app,
        json_request(
            Method::PATCH,
            settings,
            token,
            &json!({"embedding_local_dimensions": 256}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{saved}");
    assert_eq!(saved["embedding_local_dimensions"], 256);
    assert_eq!(saved["embedding_dimension"], 256);
    assert_eq!(
        saved["embedding_local_model_dimensions"]["bge-small-zh-v1.5"],
        512
    );
    let (status, _) = send_json(
        &app,
        json_request(
            Method::PATCH,
            settings,
            token,
            &json!({"embedding_local_dimensions": 513}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(manager.service().embedder().dimension(), 256);
    let restarted = Management::new(config.clone(), store).await.unwrap();
    assert_eq!(restarted.service().embedder().dimension(), 256);
    let (status, reset) = send_json(
        &app,
        request(
            Method::DELETE,
            "/admin/api/settings/embedding",
            token,
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["embedding_local_dimensions"], 0);
    assert_eq!(reset["embedding_dimension"], 512);
}

#[tokio::test]
async fn workbench_tracks_model_switches_and_reembeds_unchanged_content() {
    let Some(url) = qdrant_url() else { return };
    let dir = tempfile::tempdir().unwrap();
    let upstream = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/embeddings"))
        .respond_with(ResponseTemplate::new(200).set_body_json(json!({
            "data": [{"index": 0, "embedding": [1.0, 0.0]}]
        })))
        .mount(&upstream)
        .await;

    let mut config = AppConfig {
        qdrant: qdrant_config(&url, 45),
        ingest: ingest_config(),
        ..AppConfig::default()
    };
    config.embedding.provider = ProviderKind::Mock;
    config.embedding.mock_dimensions = 2;
    config.embedding.api.max_retries = 0;
    config.admin.access_key = "workbench-test-admin".into();
    config.admin.settings_path = dir.path().join("runtime.json");
    let admin_token = Some(config.admin.access_key.as_str());
    let store = Arc::new(QdrantStore::new(&config.qdrant).unwrap());
    let manager = Arc::new(
        Management::new(config.clone(), store.clone())
            .await
            .unwrap(),
    );
    let api = build_managed_router(
        app_state(manager.service(), Some(API_KEY), local_embedding_info(), 60),
        manager.clone(),
    );
    let workbench = kb_dashboard::build_app(
        kb_dashboard::AppState::new(store.clone(), url, move || manager.snapshot()),
        dir.path(),
    );
    let settings = "/admin/api/settings";
    for (verb, route) in [
        (Method::GET, "/api/embedding/config"),
        (Method::POST, "/api/embedding/config"),
        (Method::DELETE, "/api/embedding/config"),
        (Method::POST, "/api/embedding/mock"),
        (Method::PATCH, settings),
    ] {
        let (status, _) = send(&workbench, json_request(verb, route, None, &json!({}))).await;
        assert_eq!(status, StatusCode::NOT_FOUND, "{route}");
    }
    let (status, _) = send(
        &api,
        json_request(Method::POST, "/api/pipeline/run", None, &json!({})),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let name = unique_name("workbench_switch");
    let input = json!({
        "collection": name,
        "doc_id": "policy",
        "text": "Company travel expenses require a receipt for reimbursement."
    });
    let initial = run_workbench_pipeline(&workbench, &input).await;
    let initial_model = initial["embedding_model"].as_str().unwrap();
    assert_eq!(initial_model, "deterministic-mock");
    assert_eq!(initial["dimension"], 2);

    let mut patch = json!({
        "embedding_provider": "api",
        "embedding_api_protocol": "openai",
        "embedding_api_base_url": format!("{}/broken", upstream.uri()),
        "embedding_api_model": "test-api-v2",
        "embedding_api_dimensions": 2,
        "rerank_candidate_limit": 12
    });
    let (status, _) = send_json(
        &api,
        json_request(Method::PATCH, settings, admin_token, &patch),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_GATEWAY);
    assert!(!config.admin.settings_path.exists());

    patch["embedding_api_base_url"] = json!(format!("{}/v1", upstream.uri()));
    let (status, changed) = send_json(
        &api,
        json_request(Method::PATCH, settings, admin_token, &patch),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{changed}");
    assert_eq!(changed["embedding_model"], "test-api-v2");

    for (app, endpoint) in [(&api, "/health"), (&workbench, "/api/overview")] {
        let (status, body) = send_json(
            app,
            request(Method::GET, endpoint, None, None, Body::empty()),
        )
        .await;
        assert_eq!(status, StatusCode::OK, "{body}");
        assert_eq!(body["embedding"]["model"], "test-api-v2");
        assert_eq!(body["embedding"]["dimension"], 2);
    }

    let rewritten = run_workbench_pipeline(&workbench, &input).await;
    assert_eq!(rewritten["embedding_model"], "test-api-v2");
    assert_eq!(rewritten["skipped"], false);
    let (status, details) = send_json(
        &workbench,
        request(
            Method::GET,
            &format!("/api/collections/{name}/document?doc_id=policy"),
            None,
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{details}");
    assert_eq!(details["embedding_model"], "test-api-v2");
    let (status, results) = send_json(
        &workbench,
        json_request(
            Method::POST,
            &format!("/api/collections/{name}/search"),
            None,
            &json!({"query": "receipts for travel"}),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{results}");
    assert_eq!(results["embedding_model"], "test-api-v2");
    assert_eq!(results["results"][0]["doc_id"], "policy");

    let embed_endpoint = "/admin/api/settings/embedding";
    let probe_endpoint = format!("{embed_endpoint}/probe");
    let (status, probe) = send_json(
        &api,
        request(
            Method::POST,
            &probe_endpoint,
            admin_token,
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{probe}");
    assert_eq!(probe["sample"], json!([1.0, 0.0]));
    let restarted = Management::new(config.clone(), store.clone())
        .await
        .unwrap();
    assert_eq!(restarted.service().embedder().model_id(), "test-api-v2");

    let (status, reset) = send_json(
        &api,
        request(
            Method::DELETE,
            embed_endpoint,
            admin_token,
            None,
            Body::empty(),
        ),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reset}");
    assert_eq!(reset["embedding_model"], initial_model);
    assert_eq!(reset["rerank_candidate_limit"], 12);
    let restarted = Management::new(config, store).await.unwrap();
    assert_eq!(restarted.service().embedder().model_id(), initial_model);
    restarted.service().delete_collection(&name).await.unwrap();
}

async fn run_workbench_pipeline(workbench: &Router, input: &Value) -> Value {
    let (status, job) = send_json(
        workbench,
        json_request(Method::POST, "/api/pipeline/run", None, input),
    )
    .await;
    assert_eq!(status, StatusCode::ACCEPTED, "{job}");
    let endpoint = format!("/api/pipeline/jobs/{}", job["job_id"].as_str().unwrap());
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let (status, job) = send_json(
                workbench,
                request(Method::GET, &endpoint, None, None, Body::empty()),
            )
            .await;
            assert_eq!(status, StatusCode::OK, "{job}");
            match job["status"].as_str().unwrap() {
                "succeeded" => return job["result"].clone(),
                "failed" => panic!("pipeline failed: {job}"),
                _ => tokio::time::sleep(Duration::from_millis(30)).await,
            }
        }
    })
    .await
    .expect("pipeline did not finish")
}

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
async fn v1_body_limit_returns_413_envelope() {
    let service = service_for(UNREACHABLE_QDRANT, 1, deterministic_embedder());
    let mut state = app_state(service, None, local_embedding_info(), 10);
    state.max_body_bytes = 64;
    let app = build_router(state);

    let oversized = json!({
        "name": "docs",
        "padding": "x".repeat(1024),
    });
    let (status, body) = send_json(
        &app,
        json_request(Method::POST, "/v1/collections", None, &oversized),
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
    let service = service_for(&url, 45, deterministic_embedder());
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
    let service = service_for(&url, 45, deterministic_embedder());
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
        protocol: Default::default(),
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

    let service = service_for(&url, 45, embedder);
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
