//! Ingestion, reconciliation, cleanup and document deletion.

use std::collections::HashSet;

use qdrant_client::qdrant::{Condition, Filter, PointStruct};
use sha2::{Digest, Sha256};
use uuid::Uuid;

use super::payload::{
    chunk_payload_matches, doc_filter, hash_filter, metadata_matches, metadata_payload,
    payload_for, payload_str, to_qdrant_payload, validate_collection_name, validate_doc_id,
};
use super::KnowledgeBaseService;
use crate::error::{KbError, Result};
use crate::ingest::chunker::{chunk_document, Chunk};
use crate::ingest::parser::parse_text;
use crate::models::{DocumentFormat, DocumentMetadata, IngestResult};

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

impl KnowledgeBaseService {
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
        self.check_collection_dimension(collection).await?;

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
                // Compare the stored text, heading path and model before
                // skipping. A model switch can require new vectors even when
                // the dimensions and resulting chunk ids are unchanged.
                let chunks_match = payloads.len() == chunk_count
                    && payloads.iter().all(|payload| {
                        payload_str(payload, "embedding_model") == Some(embedding_model.as_str())
                            && chunk_payload_matches(payload, &chunks)
                    });
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

        // The stored chunks or model differ: embed and rewrite this document,
        // then clean up every stale point.
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
            must: vec![super::payload::identity_condition("doc_id", doc_id)],
            must_not: vec![Condition::has_id(
                keep.iter().map(|id| id.to_string()).collect::<Vec<_>>(),
            )],
            ..Default::default()
        };
        self.store.delete_by_filter(collection, filter).await
    }
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

fn point_ids(collection: &str, doc_id: &str, content_hash: &str, chunk_count: usize) -> Vec<Uuid> {
    (0..chunk_count)
        .map(|index| point_id(collection, doc_id, content_hash, index))
        .collect()
}

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

fn vector_probe(vector: &[f32]) -> (Vec<f32>, f32) {
    let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    (
        vector.iter().take(VECTOR_PREVIEW_DIMS).copied().collect(),
        norm,
    )
}

fn content_hash(text: &str) -> String {
    sha256_hex(text)
}

pub(super) fn sha256_hex(text: &str) -> String {
    let digest = Sha256::digest(text.as_bytes());
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn first_16(hex: &str) -> &str {
    hex.get(..16).unwrap_or(hex)
}
