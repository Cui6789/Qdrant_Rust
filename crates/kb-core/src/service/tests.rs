use std::sync::Arc;

use super::payload::{
    build_filter, metadata_from_payload, metadata_matches, payload_for, validate_collection_name,
    validate_search_request,
};
use super::retrieval::{document_summary, rerank_hits};
use super::write::{resolve_doc_id, sha256_hex, IngestInput};
use super::KnowledgeBaseService;
use crate::config::{IngestConfig, QdrantConfig};
use crate::embedding::mock::DeterministicEmbedding;
use crate::ingest::chunker::Chunk;
use crate::ingest::upload::UploadParser;
use crate::models::{SearchHit, SearchHitMetadata};
use crate::rerank::{RankedDocument, RerankProvider};
use crate::store::QdrantStore;
use qdrant_client::qdrant::condition::ConditionOneOf;
use qdrant_client::qdrant::r#match::MatchValue;
use qdrant_client::qdrant::{FieldCondition, RepeatedStrings};

fn input(doc_id: Option<&str>, source: Option<&str>) -> IngestInput {
    IngestInput {
        doc_id: doc_id.map(str::to_string),
        text: "some text".to_string(),
        format: crate::models::DocumentFormat::Text,
        metadata: crate::models::DocumentMetadata {
            source: source.map(str::to_string),
            ..Default::default()
        },
    }
}

fn field_condition<'a>(filter: &'a qdrant_client::qdrant::Filter, key: &str) -> &'a FieldCondition {
    filter
        .must
        .iter()
        .find_map(|condition| match &condition.condition_one_of {
            Some(ConditionOneOf::Field(field)) if field.key == key => Some(field),
            _ => None,
        })
        .unwrap_or_else(|| panic!("missing field condition for {key}"))
}

fn match_value(field: &FieldCondition) -> &MatchValue {
    field
        .r#match
        .as_ref()
        .expect("match must be present")
        .match_value
        .as_ref()
        .expect("match value must be present")
}

#[test]
fn runtime_snapshot_shares_store_and_write_locks() {
    let store = Arc::new(
        QdrantStore::new(&QdrantConfig {
            url: "http://localhost:6334".to_string(),
            ..QdrantConfig::default()
        })
        .expect("store builds"),
    );
    let embedder = Arc::new(DeterministicEmbedding::new(8));
    let service = KnowledgeBaseService::new(store, embedder.clone(), IngestConfig::default());
    let runtime = service.with_runtime(embedder, None, 30, Arc::new(UploadParser::default()));

    assert!(Arc::ptr_eq(&service.store, &runtime.store));
    assert!(Arc::ptr_eq(&service.locks, &runtime.locks));
    assert_eq!(runtime.embedder().dimension(), 8);
}

#[tokio::test]
async fn qdrant_browsing_uses_cursor_and_first_chunk_scan() {
    let Ok(url) = std::env::var("KB_TEST_QDRANT_URL") else {
        eprintln!("skipping: KB_TEST_QDRANT_URL not set");
        return;
    };
    let store = Arc::new(
        QdrantStore::new(&QdrantConfig {
            url,
            timeout_secs: 45,
            ..QdrantConfig::default()
        })
        .expect("store builds"),
    );
    let embedder = Arc::new(DeterministicEmbedding::new(8));
    let service = KnowledgeBaseService::new(
        store,
        embedder,
        IngestConfig {
            chunk_size: 45,
            chunk_overlap: 5,
            min_chunk_size: 1,
            upsert_batch_size: 2,
        },
    );
    let collection = format!("browse-{}", uuid::Uuid::new_v4().simple());
    service.create_collection(&collection).await.unwrap();

    for (doc_id, text) in [
        (
            "doc-a",
            "Alpha first sentence has enough words. Alpha second sentence has more words. Alpha third sentence closes the document.",
        ),
        (
            "doc-b",
            "Bravo first sentence has enough words. Bravo second sentence has more words. Bravo third sentence closes the document.",
        ),
    ] {
        service
            .ingest_document(
                &collection,
                IngestInput {
                    doc_id: Some(doc_id.to_string()),
                    text: text.to_string(),
                    format: crate::models::DocumentFormat::Text,
                    metadata: crate::models::DocumentMetadata {
                        title: Some(format!("{doc_id} title")),
                        tags: vec!["browse".to_string()],
                        ..Default::default()
                    },
                },
            )
            .await
            .unwrap();
    }

    let first = service.list_documents(&collection, 1, None).await.unwrap();
    assert_eq!(first.items.len(), 1);
    let cursor = first
        .next_cursor
        .as_deref()
        .expect("second document cursor");
    let second = service
        .list_documents(&collection, 1, Some(cursor))
        .await
        .unwrap();
    assert_eq!(second.items.len(), 1);
    assert_ne!(first.items[0].doc_id, second.items[0].doc_id);
    assert_eq!(first.items[0].tags, vec!["browse".to_string()]);

    let chunks = service
        .document_chunks(&collection, "doc-a", 2, None)
        .await
        .unwrap();
    assert!(!chunks.items.is_empty());
    assert!(chunks.items.iter().all(|chunk| !chunk.point_id.is_empty()));
    assert!(chunks.items.iter().all(|chunk| !chunk.text.is_empty()));

    service.delete_collection(&collection).await.unwrap();
}

#[test]
fn validates_collection_names() {
    for accepted in ["a", "docs-2026_01", "A0_-"] {
        assert!(
            validate_collection_name(accepted).is_ok(),
            "{accepted:?} should be accepted"
        );
    }

    let too_long = "a".repeat(65);
    for rejected in [
        "",
        "-bad",
        "_bad",
        "bad name",
        "__meta",
        "bad/name",
        too_long.as_str(),
    ] {
        assert!(
            matches!(
                validate_collection_name(rejected),
                Err(crate::error::KbError::Validation(_))
            ),
            "{rejected:?} should be rejected"
        );
    }
}

#[test]
fn resolves_doc_id_precedence() {
    let explicit = input(Some("explicit"), Some("handbook.md"));
    assert_eq!(
        resolve_doc_id(&explicit, "0123456789abcdef").unwrap(),
        "explicit"
    );

    let sourced = input(None, Some("handbook.md"));
    let source_id = resolve_doc_id(&sourced, "0123456789abcdef").unwrap();
    let expected = format!("doc-{}", &sha256_hex("handbook.md")[..16]);
    assert_eq!(source_id, expected);

    let hashed = input(None, None);
    let content_hash = "fedcba9876543210fedcba9876543210fedcba9876543210fedcba9876543210";
    assert_eq!(
        resolve_doc_id(&hashed, content_hash).unwrap(),
        "doc-fedcba9876543210"
    );
}

#[test]
fn builds_filter_for_search() {
    let filter = crate::models::SearchFilter {
        doc_ids: Some(vec!["doc-a".to_string(), "doc-b".to_string()]),
        tags_any: Some(vec!["hr".to_string(), "policy".to_string()]),
        source: Some("handbook.md".to_string()),
    };
    let built = build_filter(Some(&filter)).expect("filter is built");

    assert_eq!(built.must.len(), 3);
    assert_eq!(
        match_value(field_condition(&built, "source")),
        &MatchValue::Keyword("handbook.md".to_string())
    );
    assert_eq!(
        match_value(field_condition(&built, "doc_id")),
        &MatchValue::Keywords(RepeatedStrings {
            strings: vec!["doc-a".to_string(), "doc-b".to_string()],
        })
    );
    assert_eq!(
        match_value(field_condition(&built, "tags")),
        &MatchValue::Keywords(RepeatedStrings {
            strings: vec!["hr".to_string(), "policy".to_string()],
        })
    );

    assert!(build_filter(None).is_none());
    assert!(build_filter(Some(&crate::models::SearchFilter::default())).is_none());
}

#[test]
fn payload_roundtrip_metadata() {
    let chunk = Chunk {
        index: 2,
        text: "块正文".to_string(),
        heading_path: "第二章 > 休假制度".to_string(),
    };
    let metadata = crate::models::DocumentMetadata {
        source: Some("handbook.md".to_string()),
        title: Some("员工手册".to_string()),
        tags: vec!["hr".to_string(), "policy".to_string()],
        extra: serde_json::json!({"dept": "HR"})
            .as_object()
            .expect("object")
            .clone(),
    };

    let payload = payload_for(
        "handbook",
        "abc123",
        &chunk,
        5,
        &metadata,
        "deterministic-mock",
        "2026-11-25T12:00:00Z",
    );
    assert_eq!(
        payload.get("doc_id").and_then(serde_json::Value::as_str),
        Some("handbook")
    );
    assert_eq!(
        payload
            .get("chunk_index")
            .and_then(serde_json::Value::as_u64),
        Some(2)
    );
    assert_eq!(
        payload
            .get("chunk_count")
            .and_then(serde_json::Value::as_u64),
        Some(5)
    );
    assert_eq!(
        payload.get("text").and_then(serde_json::Value::as_str),
        Some("块正文")
    );

    let parsed = metadata_from_payload(&payload);
    assert_eq!(parsed.source.as_deref(), Some("handbook.md"));
    assert_eq!(parsed.title.as_deref(), Some("员工手册"));
    assert_eq!(parsed.tags, metadata.tags);
    assert_eq!(parsed.heading_path, "第二章 > 休假制度");
    assert_eq!(parsed.extra, metadata.extra);
    assert!(metadata_matches(&payload, &metadata));

    let bare = crate::models::DocumentMetadata::default();
    let bare_payload = payload_for(
        "doc",
        "hash",
        &Chunk {
            index: 0,
            text: "text".to_string(),
            heading_path: String::new(),
        },
        1,
        &bare,
        "mock",
        "2026-11-25T12:00:00Z",
    );
    assert!(metadata_matches(&bare_payload, &bare));
    let parsed_bare = metadata_from_payload(&bare_payload);
    assert_eq!(parsed_bare.source, None);
    assert_eq!(parsed_bare.title, None);
    assert!(parsed_bare.tags.is_empty());
    assert_eq!(parsed_bare.heading_path, "");
}

#[test]
fn document_summary_preserves_doc_id_whitespace_and_skips_missing_ids() {
    let payload = serde_json::json!({
        "doc_id": "  external/doc  ",
        "title": "External",
        "chunk_count": 7
    });
    let summary = document_summary(payload.as_object().unwrap()).expect("valid summary");
    assert_eq!(summary.doc_id, "  external/doc  ");
    assert_eq!(summary.chunk_count, 7);

    let missing = serde_json::json!({"title": "no id"});
    assert!(document_summary(missing.as_object().unwrap()).is_none());
    let blank = serde_json::json!({"doc_id": "   "});
    assert!(document_summary(blank.as_object().unwrap()).is_none());
}

#[test]
fn validates_search_limits() {
    let request = |query: &str, top_k: usize| crate::models::SearchRequest {
        query: query.to_string(),
        top_k,
        score_threshold: None,
        filter: None,
    };

    for rejected in [
        request("", 5),
        request("   ", 5),
        request("q", 0),
        request("q", 51),
    ] {
        assert!(
            matches!(
                validate_search_request(&rejected),
                Err(crate::error::KbError::Validation(_))
            ),
            "{rejected:?} should be rejected"
        );
    }
    for accepted in [request("q", 1), request("q", 50)] {
        assert!(
            validate_search_request(&accepted).is_ok(),
            "{accepted:?} should be accepted"
        );
    }
}

struct FakeReranker {
    rankings: Vec<(usize, f32)>,
    fail: bool,
}

#[async_trait::async_trait]
impl RerankProvider for FakeReranker {
    async fn rerank(
        &self,
        _query: &str,
        _documents: &[&str],
    ) -> crate::error::Result<Vec<RankedDocument>> {
        if self.fail {
            return Err(crate::error::KbError::Rerank("provider failed".to_string()));
        }
        Ok(self
            .rankings
            .iter()
            .map(|(index, score)| RankedDocument {
                index: *index,
                score: *score,
            })
            .collect())
    }
}

fn search_hit(text: &str, score: f32) -> SearchHit {
    SearchHit {
        score,
        rerank_score: None,
        doc_id: "doc".to_string(),
        chunk_index: 0,
        text: text.to_string(),
        metadata: SearchHitMetadata::default(),
    }
}

#[tokio::test]
async fn rerank_sorts_output_and_preserves_vector_ties() {
    let hits = vec![
        search_hit("zero", 0.9),
        search_hit("one", 0.8),
        search_hit("two", 0.7),
        search_hit("three", 0.6),
        search_hit("   ", 0.5),
    ];
    let reranker = FakeReranker {
        rankings: vec![(2, 0.5), (0, 0.5), (1, 0.99), (3, 0.2)],
        fail: false,
    };

    let ranked = rerank_hits("query", &reranker, hits, 10).await.unwrap();
    assert_eq!(
        ranked
            .iter()
            .map(|hit| hit.text.as_str())
            .collect::<Vec<_>>(),
        vec!["one", "zero", "two", "three"]
    );
    assert_eq!(
        ranked
            .iter()
            .map(|hit| hit.rerank_score)
            .collect::<Vec<_>>(),
        vec![Some(0.99), Some(0.5), Some(0.5), Some(0.2)]
    );
    assert_eq!(ranked[1].score, 0.9);
    assert_eq!(ranked[2].score, 0.7);
}

#[tokio::test]
async fn rerank_failure_is_propagated() {
    let reranker = FakeReranker {
        rankings: Vec::new(),
        fail: true,
    };
    let error = rerank_hits("query", &reranker, vec![search_hit("candidate", 1.0)], 5)
        .await
        .expect_err("rerank failure must propagate");
    assert!(matches!(error, crate::error::KbError::Rerank(_)));
}

#[tokio::test]
async fn rerank_rejects_malformed_rank_sets() {
    for rankings in [
        vec![(0, 1.0), (0, 0.5)],
        vec![(0, 1.0), (9, 0.5)],
        vec![(0, 1.0)],
        vec![(0, 1.0), (1, f32::NAN)],
    ] {
        let reranker = FakeReranker {
            rankings,
            fail: false,
        };
        let error = rerank_hits(
            "query",
            &reranker,
            vec![search_hit("zero", 1.0), search_hit("one", 0.9)],
            5,
        )
        .await
        .expect_err("malformed ranks must fail");
        assert!(matches!(error, crate::error::KbError::Rerank(_)));
    }
}

#[tokio::test]
async fn rerank_keeps_vector_recall_when_all_texts_are_empty() {
    let reranker = FakeReranker {
        rankings: Vec::new(),
        fail: true,
    };
    let ranked = rerank_hits(
        "query",
        &reranker,
        vec![search_hit("", 0.9), search_hit("  ", 0.8)],
        1,
    )
    .await
    .unwrap();
    assert_eq!(ranked.len(), 1);
    assert_eq!(ranked[0].score, 0.9);
    assert_eq!(ranked[0].rerank_score, None);
}
