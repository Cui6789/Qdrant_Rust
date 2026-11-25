//! Knowledge base business logic: identity, reconciliation and retrieval.
//!
//! Writes for one document are serialized by a fixed set of striped locks
//! (single-instance guarantee, see the design document). Every ingest first
//! reconciles the existing points for `doc_id + content_hash`; only a complete,
//! identical chunk set skips embedding. Cleanup runs after every successful
//! write and removes every point of the document that is not part of the
//! current version.

use std::collections::hash_map::DefaultHasher;
use std::collections::HashSet;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use qdrant_client::qdrant::r#match::MatchValue;
use qdrant_client::qdrant::{Condition, Filter, PointStruct, ScoredPoint};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use crate::config::IngestConfig;
use crate::embedding::EmbeddingProvider;
use crate::error::{KbError, Result};
use crate::ingest::chunker::{chunk_document, Chunk};
use crate::ingest::parser::parse_text;
use crate::models::{
    CollectionInfo, DocumentFormat, DocumentMetadata, EmbeddingInfo, HealthStatus, IngestResult,
    SearchFilter, SearchHit, SearchHitMetadata, SearchRequest,
};
use crate::store::QdrantStore;

/// Number of striped mutexes used to serialize per-document writes.
const LOCK_STRIPES: usize = 64;

/// Number of leading chunks summarized in [`IngestEvent::Chunked`].
const PREVIEW_CHUNKS: usize = 8;
/// Maximum characters kept in one chunk preview.
const PREVIEW_CHARS: usize = 180;
/// Number of leading vector components included in [`IngestEvent::EmbeddingFinished`].
const VECTOR_PREVIEW_DIMS: usize = 16;

/// Input for [`KnowledgeBaseService::ingest_document`].
#[derive(Debug, Clone)]
pub struct IngestInput {
    pub doc_id: Option<String>,
    pub text: String,
    pub format: DocumentFormat,
    pub metadata: DocumentMetadata,
}

/// A compact description of one chunk, suitable for a progress UI.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ChunkPreview {
    pub index: usize,
    pub characters: usize,
    pub heading_path: String,
    pub preview: String,
}

/// Progress events emitted while a document is ingested.
///
/// The service itself stays UI-agnostic: callers pass a synchronous observer
/// to [`KnowledgeBaseService::ingest_document_observed`] and decide how to
/// transport or render these events.
#[derive(Debug, Clone, PartialEq)]
pub enum IngestEvent {
    /// Parsing produced normalized text with `characters` Unicode scalar values.
    Parsed { characters: usize },
    /// Chunking produced `total` chunks; `previews` holds the leading ones.
    Chunked {
        total: usize,
        previews: Vec<ChunkPreview>,
    },
    /// Reconciliation found an identical version and skipped embedding.
    Existing { chunks: usize },
    /// Only metadata changed; payloads were updated in place.
    MetadataUpdated { chunks: usize },
    /// The embedding provider is about to be called.
    EmbeddingStarted {
        chunks: usize,
        model: String,
        dimension: usize,
    },
    /// Vectors were produced; `preview` holds the leading dimensions of the first.
    EmbeddingFinished {
        chunks: usize,
        dimension: usize,
        preview: Vec<f32>,
        norm: f32,
    },
    /// Points are being written to Qdrant.
    Writing { points: usize, batches: usize },
    /// All point batches were accepted by Qdrant.
    Written { points: usize },
    /// Stale points of the document were removed after the write.
    Cleaned,
}

/// In-process knowledge base operations over a Qdrant store.
pub struct KnowledgeBaseService {
    store: Arc<QdrantStore>,
    embedder: Arc<dyn EmbeddingProvider>,
    ingest: IngestConfig,
    locks: StripedLocks,
}

impl KnowledgeBaseService {
    /// Builds a service from its collaborators.
    pub fn new(
        store: Arc<QdrantStore>,
        embedder: Arc<dyn EmbeddingProvider>,
        ingest: IngestConfig,
    ) -> Self {
        Self {
            store,
            embedder,
            ingest,
            locks: StripedLocks::new(),
        }
    }

    /// Reports Qdrant connectivity and the active embedding configuration.
    pub async fn health(&self) -> HealthStatus {
        let (status, qdrant) = match self.store.list_collections().await {
            Ok(_) => ("ok", "ok"),
            Err(_) => ("degraded", "unavailable"),
        };
        HealthStatus {
            status: status.to_string(),
            qdrant: qdrant.to_string(),
            embedding: EmbeddingInfo {
                // The provider trait only exposes its model id, which is the
                // most specific identifier available to the service.
                provider: self.embedder.model_id().to_string(),
                model: self.embedder.model_id().to_string(),
                dimension: self.embedder.dimension(),
            },
        }
    }

    /// Lists collections with their real point counts and vector sizes.
    pub async fn list_collections(&self) -> Result<Vec<CollectionInfo>> {
        self.store.list_collections().await
    }

    /// Creates a collection sized for the active embedding provider.
    pub async fn create_collection(&self, name: &str) -> Result<CollectionInfo> {
        validate_collection_name(name)?;
        if self.store.collection_exists(name).await? {
            return Err(KbError::CollectionExists(name.to_string()));
        }
        let dimension = self.embedder.dimension() as u64;
        self.store.create_collection(name, dimension).await?;
        Ok(CollectionInfo {
            name: name.to_string(),
            points_count: 0,
            vector_size: dimension,
        })
    }

    /// Deletes an existing collection.
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        if !self.store.collection_exists(name).await? {
            return Err(KbError::CollectionNotFound(name.to_string()));
        }
        self.store.delete_collection(name).await
    }

    /// Ingests a document, reconciling any previously written version first.
    pub async fn ingest_document(
        &self,
        collection: &str,
        input: IngestInput,
    ) -> Result<IngestResult> {
        self.ingest_document_observed(collection, input, &|_| {})
            .await
    }

    /// Ingests a document and reports fine-grained progress through `observer`.
    ///
    /// The observer is called synchronously at each stage boundary and should
    /// stay cheap (for example, push into a channel or a mutex-protected log).
    /// A closure that ignores every event yields the same behavior as
    /// [`Self::ingest_document`].
    pub async fn ingest_document_observed(
        &self,
        collection: &str,
        input: IngestInput,
        observer: &(dyn Fn(IngestEvent) + Send + Sync),
    ) -> Result<IngestResult> {
        validate_collection_name(collection)?;
        if !self.store.collection_exists(collection).await? {
            return Err(KbError::CollectionNotFound(collection.to_string()));
        }

        let parsed = parse_text(&input.text, input.format)?;
        observer(IngestEvent::Parsed {
            characters: parsed.chars().count(),
        });
        let chunks = chunk_document(&parsed, input.format, &self.ingest)?;
        let chunk_count = chunks.len();
        observer(IngestEvent::Chunked {
            total: chunk_count,
            previews: chunk_previews(&chunks),
        });
        let content_hash = content_hash(&parsed);
        let doc_id = resolve_doc_id(&input, &content_hash)?;
        let ids = point_ids(collection, &doc_id, &content_hash, chunk_count);
        let embedding_model = self.embedder.model_id().to_string();

        let _guard = self.locks.lock(&format!("{collection}:{doc_id}")).await;

        let current_filter = hash_filter(&doc_id, &content_hash);
        let existing = self
            .store
            .count(collection, Some(current_filter.clone()))
            .await?;

        if existing == chunk_count as u64 {
            let existing_ids: HashSet<Uuid> = self
                .store
                .scroll_ids(collection, current_filter.clone(), chunk_count as u32 + 1)
                .await?
                .into_iter()
                .collect();
            let expected_ids: HashSet<Uuid> = ids.iter().copied().collect();
            if existing_ids == expected_ids {
                let payloads = self
                    .store
                    .scroll_payloads(collection, current_filter.clone(), chunk_count as u32 + 1)
                    .await?;
                // Point ids only encode the content hash and chunk index, so a
                // changed chunking configuration (size/overlap/min size) can
                // keep the same id set while producing different chunk text.
                // Compare the stored text and heading path before skipping.
                let chunks_match = payloads.len() == chunk_count
                    && payloads
                        .iter()
                        .all(|payload| chunk_payload_matches(payload, &chunks));
                if chunks_match {
                    observer(IngestEvent::Existing {
                        chunks: chunk_count,
                    });
                    let metadata_changed = payloads
                        .iter()
                        .any(|payload| !metadata_matches(payload, &input.metadata));
                    let metadata_updated = if metadata_changed {
                        self.store
                            .set_metadata_payload(
                                collection,
                                current_filter,
                                metadata_payload(&input.metadata),
                            )
                            .await?;
                        observer(IngestEvent::MetadataUpdated {
                            chunks: chunk_count,
                        });
                        true
                    } else {
                        false
                    };
                    self.cleanup_stale(collection, &doc_id, &ids).await?;
                    observer(IngestEvent::Cleaned);
                    return Ok(IngestResult {
                        doc_id,
                        chunks: chunk_count,
                        skipped: !metadata_updated,
                        metadata_updated,
                        content_hash,
                        embedding_model,
                    });
                }
            }
        }

        // Either the chunk set is incomplete or its ids do not line up: embed
        // and rewrite the whole version, then clean up every stale point.
        observer(IngestEvent::EmbeddingStarted {
            chunks: chunk_count,
            model: embedding_model.clone(),
            dimension: self.embedder.dimension(),
        });
        let vectors = self.embed_chunks(&chunks).await?;
        let (preview, norm) = vectors
            .first()
            .map(|vector| vector_probe(vector))
            .unwrap_or_default();
        observer(IngestEvent::EmbeddingFinished {
            chunks: chunk_count,
            dimension: vectors.first().map(Vec::len).unwrap_or(0),
            preview,
            norm,
        });
        let created_at = chrono::Utc::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, true);
        let mut points = Vec::with_capacity(chunk_count);
        for (chunk, vector) in chunks.iter().zip(vectors) {
            let payload = payload_for(
                &doc_id,
                &content_hash,
                chunk,
                chunk_count,
                &input.metadata,
                &embedding_model,
                &created_at,
            );
            points.push(PointStruct::new(
                ids[chunk.index].to_string(),
                vector,
                to_qdrant_payload(&payload)?,
            ));
        }
        let batch_size = self.ingest.upsert_batch_size.max(1);
        let batches = points.len().div_ceil(batch_size);
        observer(IngestEvent::Writing {
            points: points.len(),
            batches,
        });
        for batch in points.chunks(batch_size) {
            self.store.upsert(collection, batch.to_vec()).await?;
        }
        observer(IngestEvent::Written {
            points: chunk_count,
        });
        self.cleanup_stale(collection, &doc_id, &ids).await?;
        observer(IngestEvent::Cleaned);

        Ok(IngestResult {
            doc_id,
            chunks: chunk_count,
            skipped: false,
            metadata_updated: false,
            content_hash,
            embedding_model,
        })
    }

    /// Deletes every point belonging to `doc_id` and returns how many existed.
    pub async fn delete_document(&self, collection: &str, doc_id: &str) -> Result<u64> {
        validate_doc_id(doc_id)?;
        if !self.store.collection_exists(collection).await? {
            return Err(KbError::CollectionNotFound(collection.to_string()));
        }

        let _guard = self.locks.lock(&format!("{collection}:{doc_id}")).await;
        let filter = doc_filter(doc_id);
        let count = self.store.count(collection, Some(filter.clone())).await?;
        self.store.delete_by_filter(collection, filter).await?;
        Ok(count)
    }

    /// Embeds the query and maps Qdrant's scored points into search hits.
    pub async fn search(&self, collection: &str, request: SearchRequest) -> Result<Vec<SearchHit>> {
        validate_search_request(&request)?;
        if !self.store.collection_exists(collection).await? {
            return Err(KbError::CollectionNotFound(collection.to_string()));
        }

        let vector = self.embedder.embed_query(&request.query).await?;
        let filter = build_filter(request.filter.as_ref());
        let points = self
            .store
            .search(
                collection,
                vector,
                request.top_k as u64,
                request.score_threshold,
                filter,
            )
            .await?;
        Ok(points.into_iter().map(hit_from_scored_point).collect())
    }

    /// Embeds each chunk, prefixing the heading path when present.
    async fn embed_chunks(&self, chunks: &[Chunk]) -> Result<Vec<Vec<f32>>> {
        let texts: Vec<String> = chunks
            .iter()
            .map(|chunk| {
                if chunk.heading_path.is_empty() {
                    chunk.text.clone()
                } else {
                    format!("{}\n{}", chunk.heading_path, chunk.text)
                }
            })
            .collect();
        let vectors = self.embedder.embed_documents(&texts).await?;
        if vectors.len() != chunks.len() {
            return Err(KbError::Embedding(format!(
                "embedding provider returned {} vectors for {} chunks",
                vectors.len(),
                chunks.len()
            )));
        }
        Ok(vectors)
    }

    /// Removes points of `doc_id` that are not in `keep`.
    async fn cleanup_stale(&self, collection: &str, doc_id: &str, keep: &[Uuid]) -> Result<()> {
        let filter = Filter {
            must: vec![identity_condition("doc_id", doc_id)],
            must_not: vec![Condition::has_id(
                keep.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            )],
            ..Default::default()
        };
        self.store.delete_by_filter(collection, filter).await
    }
}

/// Fixed-stripe mutex set keyed by `collection + doc_id`.
struct StripedLocks {
    stripes: Vec<tokio::sync::Mutex<()>>,
}

impl StripedLocks {
    fn new() -> Self {
        Self {
            stripes: (0..LOCK_STRIPES)
                .map(|_| tokio::sync::Mutex::new(()))
                .collect(),
        }
    }

    async fn lock(&self, key: &str) -> tokio::sync::MutexGuard<'_, ()> {
        let mut hasher = DefaultHasher::new();
        key.hash(&mut hasher);
        let index = (hasher.finish() as usize) % self.stripes.len();
        self.stripes[index].lock().await
    }
}

/// Validates a collection name: `^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$`.
pub fn validate_collection_name(name: &str) -> Result<()> {
    let length = name.chars().count();
    if length == 0 || length > 64 {
        return Err(KbError::Validation(format!(
            "collection name must be 1-64 characters, got {length}"
        )));
    }
    let mut characters = name.chars();
    match characters.next() {
        Some(first) if first.is_ascii_alphanumeric() => {}
        _ => {
            return Err(KbError::Validation(format!(
                "collection name '{name}' must start with an ASCII letter or digit"
            )));
        }
    }
    if let Some(invalid) = characters.find(|character| {
        !character.is_ascii_alphanumeric() && *character != '_' && *character != '-'
    }) {
        return Err(KbError::Validation(format!(
            "collection name '{name}' contains invalid character '{invalid}'"
        )));
    }
    Ok(())
}

/// Resolves the document identity, preferring explicit ids and then the source.
pub fn resolve_doc_id(input: &IngestInput, content_hash: &str) -> Result<String> {
    let doc_id = match input.doc_id.as_deref() {
        Some(explicit) => explicit.to_string(),
        None => match input
            .metadata
            .source
            .as_deref()
            .filter(|source| !source.trim().is_empty())
        {
            Some(source) => format!("doc-{}", first_16(&sha256_hex(source))),
            None => format!("doc-{}", first_16(content_hash)),
        },
    };
    validate_doc_id(&doc_id)?;
    Ok(doc_id)
}

/// Deterministic UUID v5 point id for one chunk.
pub fn point_id(collection: &str, doc_id: &str, content_hash: &str, chunk_index: usize) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_OID,
        format!("{collection}:{doc_id}:{content_hash}:{chunk_index}").as_bytes(),
    )
}

/// Validates a search request's query and `top_k`.
pub fn validate_search_request(request: &SearchRequest) -> Result<()> {
    if request.query.trim().is_empty() {
        return Err(KbError::Validation(
            "search query must not be empty".to_string(),
        ));
    }
    if !(1..=50).contains(&request.top_k) {
        return Err(KbError::Validation(format!(
            "top_k must be between 1 and 50, got {}",
            request.top_k
        )));
    }
    Ok(())
}

/// Converts optional search filters into a Qdrant filter (AND across fields).
fn build_filter(filter: Option<&SearchFilter>) -> Option<Filter> {
    let filter = filter?;
    let mut conditions = Vec::new();
    if let Some(source) = filter.source.as_deref().filter(|source| !source.is_empty()) {
        conditions.push(identity_condition("source", source));
    }
    if let Some(doc_ids) = filter.doc_ids.as_ref().filter(|ids| !ids.is_empty()) {
        conditions.push(Condition::matches("doc_id", doc_ids.clone()));
    }
    if let Some(tags) = filter.tags_any.as_ref().filter(|tags| !tags.is_empty()) {
        conditions.push(Condition::matches("tags", tags.clone()));
    }
    if conditions.is_empty() {
        None
    } else {
        Some(Filter::must(conditions))
    }
}

/// Builds the full point payload described by the design (section 7.4).
fn payload_for(
    doc_id: &str,
    content_hash: &str,
    chunk: &Chunk,
    chunk_count: usize,
    metadata: &DocumentMetadata,
    embedding_model: &str,
    created_at: &str,
) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "doc_id".to_string(),
        serde_json::Value::String(doc_id.to_string()),
    );
    payload.insert(
        "chunk_index".to_string(),
        serde_json::json!(chunk.index as u64),
    );
    payload.insert(
        "chunk_count".to_string(),
        serde_json::json!(chunk_count as u64),
    );
    payload.insert(
        "text".to_string(),
        serde_json::Value::String(chunk.text.clone()),
    );
    payload.insert(
        "content_hash".to_string(),
        serde_json::Value::String(content_hash.to_string()),
    );
    if let Some(source) = &metadata.source {
        payload.insert(
            "source".to_string(),
            serde_json::Value::String(source.clone()),
        );
    }
    if let Some(title) = &metadata.title {
        payload.insert(
            "title".to_string(),
            serde_json::Value::String(title.clone()),
        );
    }
    payload.insert("tags".to_string(), string_array(&metadata.tags));
    if !chunk.heading_path.is_empty() {
        payload.insert(
            "heading_path".to_string(),
            serde_json::Value::String(chunk.heading_path.clone()),
        );
    }
    payload.insert(
        "extra".to_string(),
        serde_json::Value::Object(metadata.extra.clone()),
    );
    payload.insert(
        "embedding_model".to_string(),
        serde_json::Value::String(embedding_model.to_string()),
    );
    payload.insert(
        "created_at".to_string(),
        serde_json::Value::String(created_at.to_string()),
    );
    payload
}

/// Payload keys written by metadata-only updates. Absent values become null.
fn metadata_payload(metadata: &DocumentMetadata) -> serde_json::Map<String, serde_json::Value> {
    let mut payload = serde_json::Map::new();
    payload.insert(
        "source".to_string(),
        optional_string(metadata.source.as_deref()),
    );
    payload.insert(
        "title".to_string(),
        optional_string(metadata.title.as_deref()),
    );
    payload.insert("tags".to_string(), string_array(&metadata.tags));
    payload.insert(
        "extra".to_string(),
        serde_json::Value::Object(metadata.extra.clone()),
    );
    payload
}

/// Compares the stored metadata keys against the requested metadata.
fn metadata_matches(
    payload: &serde_json::Map<String, serde_json::Value>,
    metadata: &DocumentMetadata,
) -> bool {
    let source_matches =
        payload.get("source").and_then(serde_json::Value::as_str) == metadata.source.as_deref();
    let title_matches =
        payload.get("title").and_then(serde_json::Value::as_str) == metadata.title.as_deref();
    let tags_match = payload_tags(payload) == metadata.tags;
    let extra_matches = payload
        .get("extra")
        .and_then(serde_json::Value::as_object)
        .map_or_else(
            || metadata.extra.is_empty(),
            |extra| extra == &metadata.extra,
        );
    source_matches && title_matches && tags_match && extra_matches
}

/// Compares a stored payload's chunk text and heading path with the freshly
/// chunked values. A payload without a usable `chunk_index` never matches.
fn chunk_payload_matches(
    payload: &serde_json::Map<String, serde_json::Value>,
    chunks: &[Chunk],
) -> bool {
    let Some(chunk) = payload
        .get("chunk_index")
        .and_then(serde_json::Value::as_u64)
        .and_then(|index| usize::try_from(index).ok())
        .and_then(|index| chunks.get(index))
    else {
        return false;
    };
    payload.get("text").and_then(serde_json::Value::as_str) == Some(chunk.text.as_str())
        && payload
            .get("heading_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            == chunk.heading_path.as_str()
}

/// Extracts the search-hit metadata from a stored payload without panicking.
fn metadata_from_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> SearchHitMetadata {
    SearchHitMetadata {
        source: payload
            .get("source")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        title: payload
            .get("title")
            .and_then(serde_json::Value::as_str)
            .map(str::to_string),
        tags: payload_tags(payload),
        heading_path: payload
            .get("heading_path")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        extra: payload
            .get("extra")
            .and_then(serde_json::Value::as_object)
            .cloned()
            .unwrap_or_default(),
    }
}

/// Maps a scored point into a [`SearchHit`], falling back to empty values.
fn hit_from_scored_point(point: ScoredPoint) -> SearchHit {
    let payload: serde_json::Map<String, serde_json::Value> = point
        .payload
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::from(value)))
        .collect();
    SearchHit {
        score: point.score,
        doc_id: payload
            .get("doc_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        chunk_index: payload
            .get("chunk_index")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0),
        text: payload
            .get("text")
            .and_then(serde_json::Value::as_str)
            .unwrap_or_default()
            .to_string(),
        metadata: metadata_from_payload(&payload),
    }
}

/// Returns the tags stored in a payload, treating a missing field as empty.
fn payload_tags(payload: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
    payload
        .get("tags")
        .and_then(serde_json::Value::as_array)
        .map(|tags| {
            tags.iter()
                .filter_map(serde_json::Value::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default()
}

/// A keyword condition that matches a payload string exactly.
fn identity_condition(field: &str, value: &str) -> Condition {
    Condition::matches(field, MatchValue::Keyword(value.to_string()))
}

/// Filter selecting every point of one document.
fn doc_filter(doc_id: &str) -> Filter {
    Filter::must([identity_condition("doc_id", doc_id)])
}

/// Filter selecting one document version by content hash.
fn hash_filter(doc_id: &str, content_hash: &str) -> Filter {
    Filter::must([
        identity_condition("doc_id", doc_id),
        identity_condition("content_hash", content_hash),
    ])
}

/// Computes the deterministic point ids for every chunk index.
fn point_ids(collection: &str, doc_id: &str, content_hash: &str, chunk_count: usize) -> Vec<Uuid> {
    (0..chunk_count)
        .map(|index| point_id(collection, doc_id, content_hash, index))
        .collect()
}

/// Builds compact previews for the leading chunks.
fn chunk_previews(chunks: &[Chunk]) -> Vec<ChunkPreview> {
    chunks
        .iter()
        .take(PREVIEW_CHUNKS)
        .map(|chunk| ChunkPreview {
            index: chunk.index,
            characters: chunk.text.chars().count(),
            heading_path: chunk.heading_path.clone(),
            preview: truncate_chars(&chunk.text, PREVIEW_CHARS),
        })
        .collect()
}

/// Truncates `text` to at most `limit` characters, marking elision with an ellipsis.
fn truncate_chars(text: &str, limit: usize) -> String {
    let mut output = String::new();
    for (position, character) in text.chars().enumerate() {
        if position >= limit {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}

/// Returns the leading dimensions of `vector` and its L2 norm.
fn vector_probe(vector: &[f32]) -> (Vec<f32>, f32) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    (
        vector.iter().take(VECTOR_PREVIEW_DIMS).copied().collect(),
        norm,
    )
}

/// SHA-256 of normalized document text, lowercase hex.
fn content_hash(text: &str) -> String {
    sha256_hex(text)
}

/// SHA-256 of `text`, lowercase hex.
fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// First 16 characters of a hex digest.
fn first_16(hex: &str) -> &str {
    hex.get(..16).unwrap_or(hex)
}

/// Validates a document id: 1-128 characters, non-blank, no control characters.
fn validate_doc_id(doc_id: &str) -> Result<()> {
    let length = doc_id.chars().count();
    if length == 0 || length > 128 {
        return Err(KbError::Validation(format!(
            "doc_id must be 1-128 characters, got {length}"
        )));
    }
    if doc_id.trim().is_empty() {
        return Err(KbError::Validation("doc_id must not be blank".to_string()));
    }
    if doc_id.chars().any(char::is_control) {
        return Err(KbError::Validation(
            "doc_id must not contain control characters".to_string(),
        ));
    }
    Ok(())
}

/// Converts a JSON payload object into a Qdrant payload.
fn to_qdrant_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<qdrant_client::Payload> {
    qdrant_client::Payload::try_from(serde_json::Value::Object(payload.clone()))
        .map_err(|error| KbError::Qdrant(format!("failed to encode point payload: {error}")))
}

/// Builds a JSON array of strings from a tag list.
fn string_array(values: &[String]) -> serde_json::Value {
    serde_json::Value::Array(
        values
            .iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect(),
    )
}

/// Converts an optional string into a JSON string or null.
fn optional_string(value: Option<&str>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |value| {
        serde_json::Value::String(value.to_string())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
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

    fn field_condition<'a>(filter: &'a Filter, key: &str) -> &'a FieldCondition {
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
}
