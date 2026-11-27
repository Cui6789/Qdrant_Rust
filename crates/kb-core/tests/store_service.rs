use std::collections::HashSet;
use std::sync::Arc;

use kb_core::config::{IngestConfig, QdrantConfig};
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::error::KbError;
use kb_core::ingest::chunker::chunk_document;
use kb_core::ingest::parser::parse_text;
use kb_core::models::{DocumentFormat, DocumentMetadata, SearchFilter, SearchRequest};
use kb_core::service::{point_id, IngestInput, KnowledgeBaseService};
use kb_core::store::QdrantStore;
use qdrant_client::qdrant::r#match::MatchValue;
use qdrant_client::qdrant::{Condition, Filter, PointStruct};
use sha2::{Digest, Sha256};
use uuid::Uuid;

fn qdrant_url() -> Option<String> {
    match std::env::var("KB_TEST_QDRANT_URL") {
        Ok(v) if !v.is_empty() => Some(v),
        _ => {
            eprintln!("skipping: KB_TEST_QDRANT_URL not set");
            None
        }
    }
}

fn store(url: &str) -> QdrantStore {
    QdrantStore::new(&QdrantConfig {
        url: url.to_string(),
        api_key: String::new(),
        timeout_secs: 45,
        startup_retry_secs: 30,
    })
    .expect("store builds")
}

fn ingest_config() -> IngestConfig {
    IngestConfig {
        chunk_size: 40,
        chunk_overlap: 8,
        min_chunk_size: 1,
        upsert_batch_size: 2,
    }
}

fn service(
    store: &Arc<QdrantStore>,
    dimension: usize,
) -> (KnowledgeBaseService, Arc<DeterministicEmbedding>) {
    let embedder = Arc::new(DeterministicEmbedding::new(dimension));
    let service = KnowledgeBaseService::new(store.clone(), embedder.clone(), ingest_config());
    (service, embedder)
}

fn unique_name(prefix: &str) -> String {
    format!("t{prefix}-{}", Uuid::new_v4().simple())
}

fn keyword(field: &str, value: &str) -> Condition {
    Condition::matches(field, MatchValue::Keyword(value.to_string()))
}

fn doc_filter(doc_id: &str) -> Filter {
    Filter::must([keyword("doc_id", doc_id)])
}

fn hash_filter(doc_id: &str, content_hash: &str) -> Filter {
    Filter::must([
        keyword("doc_id", doc_id),
        keyword("content_hash", content_hash),
    ])
}

fn content_hash(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn long_text(seed: &str) -> String {
    let mut text = String::new();
    for index in 0..8 {
        text.push_str(&format!(
            "{seed} sentence {index} carries enough words to fill a chunk. "
        ));
    }
    text
}

fn fake_payload(
    doc_id: &str,
    content_hash: &str,
    chunk_index: u64,
    chunk_count: u64,
) -> qdrant_client::Payload {
    qdrant_client::Payload::try_from(serde_json::json!({
        "doc_id": doc_id,
        "content_hash": content_hash,
        "chunk_index": chunk_index,
        "chunk_count": chunk_count,
        "text": "fake point",
        "tags": [],
        "extra": {},
        "embedding_model": "deterministic-mock",
        "created_at": "2026-11-25T00:00:00Z"
    }))
    .expect("payload converts")
}

fn vector(dimension: usize) -> Vec<f32> {
    vec![0.1_f32; dimension]
}

#[tokio::test]
async fn lifecycle_create_ingest_search_delete() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("life");

    service.create_collection(&name).await.expect("create");
    let metadata = DocumentMetadata {
        source: Some("handbook.md".to_string()),
        title: Some("Employee Handbook".to_string()),
        tags: vec!["hr".to_string(), "policy".to_string()],
        extra: serde_json::json!({"dept": "HR"})
            .as_object()
            .expect("object")
            .clone(),
    };
    let text = "# Employee Handbook\n\nAnnual leave can be requested in the HR system.";

    let first = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some("handbook".to_string()),
                text: text.to_string(),
                format: DocumentFormat::Markdown,
                metadata: metadata.clone(),
            },
        )
        .await
        .expect("ingest");
    assert!(!first.skipped);
    assert!(!first.metadata_updated);
    assert!(first.chunks >= 1);

    let hits = service
        .search(
            &name,
            SearchRequest {
                query: "annual leave".to_string(),
                top_k: 5,
                score_threshold: None,
                filter: None,
            },
        )
        .await
        .expect("search");
    assert!(!hits.is_empty(), "expected at least one hit");
    assert!(
        hits.iter().any(|hit| hit.text.contains("Annual leave")),
        "expected the ingested text in a hit: {hits:?}"
    );
    let hit = hits
        .iter()
        .find(|hit| hit.doc_id == "handbook")
        .expect("handbook hit");
    assert_eq!(hit.metadata.source.as_deref(), Some("handbook.md"));
    assert_eq!(hit.metadata.title.as_deref(), Some("Employee Handbook"));
    assert_eq!(
        hit.metadata.tags,
        vec!["hr".to_string(), "policy".to_string()]
    );
    assert_eq!(hit.metadata.extra, metadata.extra);

    let second = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some("handbook".to_string()),
                text: text.to_string(),
                format: DocumentFormat::Markdown,
                metadata: metadata.clone(),
            },
        )
        .await
        .expect("re-ingest");
    assert!(second.skipped);
    assert!(!second.metadata_updated);

    let deleted = service
        .delete_document(&name, "handbook")
        .await
        .expect("delete document");
    assert!(deleted >= 1);
    assert_eq!(store.count(&name, None).await.expect("count"), 0);

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
    assert!(!store
        .collection_exists(&name)
        .await
        .expect("collection_exists"));
}

#[tokio::test]
async fn reinject_repairs_missing_points_and_stale_tail() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("repair");
    service.create_collection(&name).await.expect("create");

    let doc_id = "doc-repair";
    let text = long_text("repair");
    let first = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text.clone(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata::default(),
            },
        )
        .await
        .expect("ingest");
    assert!(
        first.chunks >= 2,
        "need several chunks, got {}",
        first.chunks
    );
    let hash = first.content_hash.clone();
    let expected: HashSet<Uuid> = (0..first.chunks)
        .map(|index| point_id(&name, doc_id, &hash, index))
        .collect();

    // Delete one real point and add a fake point with the same identity, so the
    // row count still equals the chunk count.
    let missing = point_id(&name, doc_id, &hash, 0);
    store
        .delete_by_filter(
            &name,
            Filter::must([Condition::has_id([missing.to_string()])]),
        )
        .await
        .expect("delete one point");
    let fake_id = Uuid::new_v4();
    store
        .upsert(
            &name,
            vec![PointStruct::new(
                fake_id.to_string(),
                vector(8),
                fake_payload(doc_id, &hash, 999, first.chunks as u64),
            )],
        )
        .await
        .expect("upsert fake");
    assert_eq!(
        store
            .count(&name, Some(hash_filter(doc_id, &hash)))
            .await
            .expect("count"),
        first.chunks as u64
    );

    let repaired = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text.clone(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata::default(),
            },
        )
        .await
        .expect("repair");
    assert!(!repaired.skipped, "repair must not be reported as skipped");
    let ids: HashSet<Uuid> = store
        .scroll_ids(&name, doc_filter(doc_id), 1000)
        .await
        .expect("scroll ids")
        .into_iter()
        .collect();
    assert_eq!(ids, expected);
    assert!(!ids.contains(&fake_id));
    assert_eq!(
        store
            .count(&name, Some(doc_filter(doc_id)))
            .await
            .expect("count"),
        first.chunks as u64
    );

    // Now add a same-hash tail point without deleting anything.
    let tail_id = Uuid::new_v4();
    store
        .upsert(
            &name,
            vec![PointStruct::new(
                tail_id.to_string(),
                vector(8),
                fake_payload(doc_id, &hash, 999, first.chunks as u64),
            )],
        )
        .await
        .expect("upsert tail");
    assert_eq!(
        store
            .count(&name, Some(doc_filter(doc_id)))
            .await
            .expect("count"),
        first.chunks as u64 + 1
    );

    let repaired_tail = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text,
                format: DocumentFormat::Text,
                metadata: DocumentMetadata::default(),
            },
        )
        .await
        .expect("repair tail");
    assert!(!repaired_tail.skipped);
    assert_eq!(
        store
            .count(&name, Some(doc_filter(doc_id)))
            .await
            .expect("count"),
        first.chunks as u64
    );
    let ids_after: HashSet<Uuid> = store
        .scroll_ids(&name, doc_filter(doc_id), 1000)
        .await
        .expect("scroll ids")
        .into_iter()
        .collect();
    assert_eq!(ids_after, expected);

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn concurrent_ingests_of_same_doc_leave_one_complete_version() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("concurrent");
    service.create_collection(&name).await.expect("create");

    let doc_id = "doc-concurrent";
    let text_a = long_text("alpha");
    let text_b = long_text("bravo");
    let metadata = DocumentMetadata::default();

    let (a, b) = tokio::join!(
        service.ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text_a,
                format: DocumentFormat::Text,
                metadata: metadata.clone(),
            },
        ),
        service.ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text_b,
                format: DocumentFormat::Text,
                metadata: metadata.clone(),
            },
        ),
    );
    let a = a.expect("first ingest");
    let b = b.expect("second ingest");

    let payloads = store
        .scroll_payloads(&name, doc_filter(doc_id), 1000)
        .await
        .expect("scroll payloads");
    let hashes: HashSet<String> = payloads
        .iter()
        .filter_map(|payload| {
            payload
                .get("content_hash")
                .and_then(serde_json::Value::as_str)
                .map(str::to_string)
        })
        .collect();
    assert_eq!(
        hashes.len(),
        1,
        "exactly one content hash should remain, got {hashes:?}"
    );
    let remaining = hashes.into_iter().next().expect("one hash");
    let expected_chunks = if remaining == a.content_hash {
        a.chunks
    } else {
        b.chunks
    };
    assert!(expected_chunks >= 2, "need several chunks");
    assert_eq!(payloads.len(), expected_chunks);

    let expected: HashSet<Uuid> = (0..expected_chunks)
        .map(|index| point_id(&name, doc_id, &remaining, index))
        .collect();
    let actual: HashSet<Uuid> = store
        .scroll_ids(&name, doc_filter(doc_id), 1000)
        .await
        .expect("scroll ids")
        .into_iter()
        .collect();
    assert_eq!(actual, expected);

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn reinject_after_partial_write_converges() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("partial");
    service.create_collection(&name).await.expect("create");

    let doc_id = "doc-partial";
    let text_a = long_text("old-version");
    let text_b = long_text("new-version");
    let metadata = DocumentMetadata::default();

    let version_a = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text_a,
                format: DocumentFormat::Text,
                metadata: metadata.clone(),
            },
        )
        .await
        .expect("ingest version A");

    let parsed_b = parse_text(&text_b, DocumentFormat::Text).expect("parse version B");
    let hash_b = content_hash(&parsed_b);
    let chunks_b =
        chunk_document(&parsed_b, DocumentFormat::Text, &ingest_config()).expect("chunk version B");
    assert!(chunks_b.len() >= 2, "need several chunks");

    // Simulate an interrupted write: only the first point of version B exists.
    let first_id = point_id(&name, doc_id, &hash_b, 0);
    store
        .upsert(
            &name,
            vec![PointStruct::new(
                first_id.to_string(),
                vector(8),
                fake_payload(doc_id, &hash_b, 0, chunks_b.len() as u64),
            )],
        )
        .await
        .expect("partial upsert");
    assert_eq!(
        store
            .count(&name, Some(hash_filter(doc_id, &hash_b)))
            .await
            .expect("count"),
        1
    );

    let repaired = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text_b,
                format: DocumentFormat::Text,
                metadata,
            },
        )
        .await
        .expect("re-ingest version B");
    assert!(!repaired.skipped);

    let expected: HashSet<Uuid> = (0..repaired.chunks)
        .map(|index| point_id(&name, doc_id, &repaired.content_hash, index))
        .collect();
    let actual: HashSet<Uuid> = store
        .scroll_ids(&name, doc_filter(doc_id), 1000)
        .await
        .expect("scroll ids")
        .into_iter()
        .collect();
    assert_eq!(actual, expected);
    assert_eq!(
        store
            .count(&name, Some(hash_filter(doc_id, &version_a.content_hash)))
            .await
            .expect("count old version"),
        0
    );

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn metadata_only_change_updates_payload_without_embedding() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, embedder) = service(&store, 8);
    let name = unique_name("meta");
    service.create_collection(&name).await.expect("create");

    let doc_id = "doc-meta";
    let text = "metadata only change test text";
    service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text.to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata {
                    tags: vec!["old".to_string()],
                    ..Default::default()
                },
            },
        )
        .await
        .expect("ingest");

    let calls_before = embedder.call_count();
    let updated = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: text.to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata {
                    tags: vec!["new".to_string()],
                    ..Default::default()
                },
            },
        )
        .await
        .expect("metadata update");
    assert!(updated.metadata_updated);
    assert!(!updated.skipped);
    assert_eq!(embedder.call_count(), calls_before);

    let hits = service
        .search(
            &name,
            SearchRequest {
                query: text.to_string(),
                top_k: 5,
                score_threshold: None,
                filter: Some(SearchFilter {
                    tags_any: Some(vec!["new".to_string()]),
                    ..Default::default()
                }),
            },
        )
        .await
        .expect("search by new tag");
    assert!(!hits.is_empty(), "expected the re-tagged point");
    assert!(hits
        .iter()
        .all(|hit| hit.metadata.tags.iter().any(|tag| tag == "new")));

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn changing_chunk_overlap_re_embeds_instead_of_skipping() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let embedder = Arc::new(DeterministicEmbedding::new(8));
    let name = unique_name("rechunk");
    let first_config = IngestConfig {
        chunk_size: 20,
        chunk_overlap: 0,
        min_chunk_size: 1,
        upsert_batch_size: 8,
    };
    let second_config = IngestConfig {
        chunk_overlap: 5,
        ..first_config.clone()
    };
    let first_service =
        KnowledgeBaseService::new(store.clone(), embedder.clone(), first_config.clone());
    let second_service =
        KnowledgeBaseService::new(store.clone(), embedder.clone(), second_config.clone());
    first_service
        .create_collection(&name)
        .await
        .expect("create");

    // Two 15-char paragraphs: with chunk_size 20 both overlap settings pack two
    // chunks, but the second chunk's text differs (it starts with the overlap).
    let text = format!("{}\n\n{}", "a".repeat(15), "b".repeat(15));
    let doc_id = "doc-rechunk";
    let input = |text: &str| IngestInput {
        doc_id: Some(doc_id.to_string()),
        text: text.to_string(),
        format: DocumentFormat::Text,
        metadata: DocumentMetadata::default(),
    };

    let first = first_service
        .ingest_document(&name, input(&text))
        .await
        .expect("first ingest");
    assert!(!first.skipped);
    assert_eq!(first.chunks, 2, "fixture must keep the chunk count stable");

    let parsed = parse_text(&text, DocumentFormat::Text).expect("parse fixture");
    let old_chunks = chunk_document(&parsed, DocumentFormat::Text, &first_config)
        .expect("chunk with the first config");
    let new_chunks = chunk_document(&parsed, DocumentFormat::Text, &second_config)
        .expect("chunk with the second config");
    assert_eq!(old_chunks.len(), new_chunks.len());
    assert_ne!(
        old_chunks[1].text, new_chunks[1].text,
        "fixture must produce different chunk text"
    );

    let calls_before = embedder.call_count();
    let second = second_service
        .ingest_document(&name, input(&text))
        .await
        .expect("second ingest");
    assert!(
        !second.skipped,
        "a chunking-config change must not be reported as skipped"
    );
    assert!(!second.metadata_updated);
    assert!(
        embedder.call_count() > calls_before,
        "the changed chunks must be re-embedded"
    );

    let payloads = store
        .scroll_payloads(&name, doc_filter(doc_id), 100)
        .await
        .expect("scroll payloads");
    let chunk_one = payloads
        .iter()
        .find(|payload| {
            payload
                .get("chunk_index")
                .and_then(serde_json::Value::as_u64)
                == Some(1)
        })
        .expect("chunk index 1 payload");
    assert_eq!(
        chunk_one.get("text").and_then(serde_json::Value::as_str),
        Some(new_chunks[1].text.as_str()),
        "stored text must reflect the new chunking"
    );

    second_service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn delete_document_with_unicode_and_slash_ids() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("unicode");
    service.create_collection(&name).await.expect("create");

    let doc_id = "手册/2026 版";
    let ingested = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some(doc_id.to_string()),
                text: "unicode doc id body".to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata::default(),
            },
        )
        .await
        .expect("ingest");
    assert!(ingested.chunks >= 1);

    let deleted = service
        .delete_document(&name, doc_id)
        .await
        .expect("delete document");
    assert!(deleted >= 1);
    assert_eq!(store.count(&name, None).await.expect("count"), 0);

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn missing_collection_maps_to_not_found() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("missing");
    if store
        .collection_exists(&name)
        .await
        .expect("collection_exists")
    {
        store
            .delete_collection(&name)
            .await
            .expect("delete stale collection");
    }

    let search = service
        .search(
            &name,
            SearchRequest {
                query: "anything".to_string(),
                top_k: 1,
                score_threshold: None,
                filter: None,
            },
        )
        .await;
    assert!(matches!(search, Err(KbError::CollectionNotFound(_))));

    let ingest = service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some("doc".to_string()),
                text: "body".to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata::default(),
            },
        )
        .await;
    assert!(matches!(ingest, Err(KbError::CollectionNotFound(_))));

    let delete = service.delete_document(&name, "doc").await;
    assert!(matches!(delete, Err(KbError::CollectionNotFound(_))));
}

#[tokio::test]
async fn dimension_mismatch_maps_to_409_error() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (original, _) = service(&store, 4);
    let (changed, embedder) = service(&store, 8);
    let name = unique_name("dim");
    store.create_collection(&name, 4).await.expect("create");

    let input = IngestInput {
        doc_id: Some("doc-dim".to_string()),
        text: "dimension mismatch test text".to_string(),
        format: DocumentFormat::Text,
        metadata: DocumentMetadata::default(),
    };
    original
        .ingest_document(&name, input.clone())
        .await
        .unwrap();
    assert!(matches!(
        changed.ingest_document(&name, input).await,
        Err(KbError::DimensionMismatch(_))
    ));
    assert!(matches!(
        changed
            .search(
                &name,
                SearchRequest {
                    query: "dimension mismatch".into(),
                    top_k: 1,
                    score_threshold: None,
                    filter: None,
                },
            )
            .await,
        Err(KbError::DimensionMismatch(_))
    ));
    assert_eq!(embedder.call_count(), 0);

    store
        .delete_collection(&name)
        .await
        .expect("delete collection");
}

#[tokio::test]
async fn search_filter_limits_results() {
    let Some(url) = qdrant_url() else { return };
    let store = Arc::new(store(&url));
    let (service, _embedder) = service(&store, 8);
    let name = unique_name("filter");
    service.create_collection(&name).await.expect("create");

    service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some("doc-alpha".to_string()),
                text: "alpha document about leave".to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata {
                    tags: vec!["alpha".to_string()],
                    ..Default::default()
                },
            },
        )
        .await
        .expect("ingest alpha");
    service
        .ingest_document(
            &name,
            IngestInput {
                doc_id: Some("doc-beta".to_string()),
                text: "beta document about travel".to_string(),
                format: DocumentFormat::Text,
                metadata: DocumentMetadata {
                    tags: vec!["beta".to_string()],
                    ..Default::default()
                },
            },
        )
        .await
        .expect("ingest beta");

    let by_tag = service
        .search(
            &name,
            SearchRequest {
                query: "document".to_string(),
                top_k: 10,
                score_threshold: None,
                filter: Some(SearchFilter {
                    tags_any: Some(vec!["alpha".to_string()]),
                    ..Default::default()
                }),
            },
        )
        .await
        .expect("search by tag");
    assert!(!by_tag.is_empty());
    assert!(by_tag.iter().all(|hit| hit.doc_id == "doc-alpha"));

    let by_id = service
        .search(
            &name,
            SearchRequest {
                query: "document".to_string(),
                top_k: 10,
                score_threshold: None,
                filter: Some(SearchFilter {
                    doc_ids: Some(vec!["doc-beta".to_string()]),
                    ..Default::default()
                }),
            },
        )
        .await
        .expect("search by doc id");
    assert!(!by_id.is_empty());
    assert!(by_id.iter().all(|hit| hit.doc_id == "doc-beta"));

    service
        .delete_collection(&name)
        .await
        .expect("delete collection");
}
