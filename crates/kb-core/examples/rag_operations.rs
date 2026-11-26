//! Direct RAG database operations against a local Qdrant instance.
//!
//! Start Qdrant on gRPC port 6334, then run:
//!
//! ```powershell
//! cargo run -p kb-core --example rag_operations
//! ```
//!
//! The example uses the deterministic mock embedder so it runs offline. It
//! creates a temporary collection, ingests a document, searches it, lists its
//! documents through a raw scroll, and deletes everything at the end.

use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use kb_core::config::{IngestConfig, QdrantConfig};
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::models::{DocumentFormat, DocumentMetadata, SearchFilter, SearchRequest};
use kb_core::service::{IngestInput, KnowledgeBaseService};
use kb_core::store::{Condition, Filter, MatchValue, QdrantStore};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    // 1. Connect. `QdrantConfig::default()` points at http://localhost:6334.
    let store = Arc::new(QdrantStore::new(&QdrantConfig::default())?);
    store.wait_ready(Duration::from_secs(10)).await?;

    // Use the offline deterministic embedder for this runnable example. Swap it
    // for ApiEmbedding or LocalEmbedding to use real vectors.
    let embedder = Arc::new(DeterministicEmbedding::new(64));
    let service = KnowledgeBaseService::new(store.clone(), embedder, IngestConfig::default());

    let collection = format!("rag-example-{}", std::process::id());
    if store.collection_exists(&collection).await? {
        service.delete_collection(&collection).await?;
    }

    // 2. Create a collection sized for the embedder.
    let info = service.create_collection(&collection).await?;
    println!(
        "created collection {} (dimension {})",
        info.name, info.vector_size
    );

    // 3. Ingest a document. `doc_id` is stable; a second run reconciles it.
    let document = IngestInput {
        doc_id: Some("handbook".to_string()),
        text: "# 员工手册\n\n年假需要在 HR 系统提交申请，审批通过后即可使用。\n\n## 病假\n\n病假需要提供医院证明。".to_string(),
        format: DocumentFormat::Markdown,
        metadata: DocumentMetadata {
            source: Some("handbook.md".to_string()),
            title: Some("员工手册".to_string()),
            tags: vec!["hr".to_string(), "policy".to_string()],
            extra: serde_json::Map::new(),
        },
    };
    let first = service
        .ingest_document(&collection, document.clone())
        .await?;
    println!(
        "ingested doc_id={} chunks={} skipped={} metadata_updated={}",
        first.doc_id, first.chunks, first.skipped, first.metadata_updated
    );

    // A repeated ingest with identical text and metadata should skip embedding.
    let second = service
        .ingest_document(&collection, document.clone())
        .await?;
    println!("re-ingest skipped={}", second.skipped);

    // A metadata-only change updates payloads without re-embedding.
    let mut metadata_update = document;
    metadata_update.metadata.tags.push("updated".to_string());
    let updated = service
        .ingest_document(&collection, metadata_update)
        .await?;
    println!("metadata_updated={}", updated.metadata_updated);

    // 4. Semantic search. The mock embedder is deterministic, not semantic.
    let hits = service
        .search(
            &collection,
            SearchRequest {
                query: "年假怎么申请".to_string(),
                top_k: 3,
                score_threshold: None,
                filter: Some(SearchFilter {
                    doc_ids: Some(vec!["handbook".to_string()]),
                    tags_any: None,
                    source: None,
                }),
            },
        )
        .await?;
    for hit in &hits {
        println!(
            "hit score={:.4} doc_id={} chunk={} text={}",
            hit.score,
            hit.doc_id,
            hit.chunk_index,
            hit.text.replace('\n', " ")
        );
    }

    // 5. Direct DB read: scroll payloads and aggregate documents by doc_id.
    let points = store.scroll_points(&collection, None, 1_000).await?;
    let mut documents: BTreeMap<String, usize> = BTreeMap::new();
    for point in &points {
        if let Some(doc_id) = point.payload.get("doc_id").and_then(|value| value.as_str()) {
            *documents.entry(doc_id.to_string()).or_default() += 1;
        }
    }
    println!(
        "points={} documents={:?}",
        store.count(&collection, None).await?,
        documents
    );

    // 6. Direct DB filter: build a Qdrant filter without importing qdrant-client.
    let filter = Filter::must([Condition::matches(
        "doc_id",
        MatchValue::Keyword("handbook".to_string()),
    )]);
    println!(
        "handbook points={}",
        store.count(&collection, Some(filter)).await?
    );

    // 7. Delete one document, then remove the temporary collection.
    let deleted = service.delete_document(&collection, "handbook").await?;
    println!("deleted points={deleted}");
    service.delete_collection(&collection).await?;
    println!("deleted collection {collection}");

    Ok(())
}
