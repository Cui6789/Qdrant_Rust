//! Vector retrieval, reranking and cursor-based document browsing.

use std::collections::HashSet;

use qdrant_client::qdrant::{Condition, Filter};

use super::payload::{
    build_filter, doc_filter, hit_from_scored_point, payload_extra, payload_str, payload_tags,
    payload_u64, validate_collection_name, validate_doc_id, validate_search_request,
};
use super::KnowledgeBaseService;
use crate::error::{KbError, Result};
use crate::models::{
    CollectionInfo, DocumentChunk, DocumentSummary, Page, SearchHit, SearchRequest,
};
use crate::rerank::{RankedDocument, RerankProvider};
use crate::store::ScrollPayloadSelection;

const MAX_PAGE_LIMIT: u32 = 100;
const DOCUMENT_SUMMARY_FIELDS: &[&str] = &[
    "doc_id",
    "title",
    "source",
    "tags",
    "chunk_count",
    "embedding_model",
    "created_at",
];
const DOCUMENT_CHUNK_FIELDS: &[&str] = &["chunk_index", "text", "heading_path", "extra"];

impl KnowledgeBaseService {
    /// Returns collection metadata without changing its current state.
    pub async fn collection_info(&self, collection: &str) -> Result<CollectionInfo> {
        self.store.collection_info(collection).await
    }

    /// Embeds the query, optionally reranks candidates and maps them to hits.
    pub async fn search(&self, collection: &str, request: SearchRequest) -> Result<Vec<SearchHit>> {
        validate_search_request(&request)?;
        self.check_collection_dimension(collection).await?;

        let vector = self.embedder.embed_query(&request.query).await?;
        let filter = build_filter(request.filter.as_ref());
        let qdrant_limit = if self.reranker.is_some() {
            (request.top_k as u64).max(self.candidate_limit)
        } else {
            request.top_k as u64
        };
        let points = self
            .store
            .search(
                collection,
                vector,
                qdrant_limit,
                request.score_threshold,
                filter,
            )
            .await?;
        let hits: Vec<SearchHit> = points.into_iter().map(hit_from_scored_point).collect();
        match self.reranker.as_deref() {
            Some(reranker) => rerank_hits(&request.query, reranker, hits, request.top_k).await,
            None => Ok(hits),
        }
    }

    /// Lists one summary per document from its first native chunk.
    pub async fn list_documents(
        &self,
        collection: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<DocumentSummary>> {
        validate_collection_name(collection)?;
        validate_page_limit(limit)?;
        let filter = Filter::must([Condition::matches("chunk_index", 0_i64)]);
        let page = self
            .store
            .scroll_page(
                collection,
                Some(filter),
                limit,
                cursor,
                ScrollPayloadSelection::Include(DOCUMENT_SUMMARY_FIELDS),
            )
            .await?;
        let items = page
            .points
            .into_iter()
            .filter_map(|point| document_summary(&point.payload))
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }

    /// Lists the chunks of one document using Qdrant's cursor.
    pub async fn document_chunks(
        &self,
        collection: &str,
        doc_id: &str,
        limit: u32,
        cursor: Option<&str>,
    ) -> Result<Page<DocumentChunk>> {
        validate_collection_name(collection)?;
        validate_doc_id(doc_id)?;
        validate_page_limit(limit)?;
        let page = self
            .store
            .scroll_page(
                collection,
                Some(doc_filter(doc_id)),
                limit,
                cursor,
                ScrollPayloadSelection::Include(DOCUMENT_CHUNK_FIELDS),
            )
            .await?;
        let items = page
            .points
            .into_iter()
            .filter(|point| !point.id.is_empty())
            .map(|point| DocumentChunk {
                point_id: point.id,
                chunk_index: payload_u64(&point.payload, "chunk_index").unwrap_or(0),
                text: payload_str(&point.payload, "text")
                    .unwrap_or_default()
                    .to_string(),
                heading_path: payload_str(&point.payload, "heading_path")
                    .unwrap_or_default()
                    .to_string(),
                extra: payload_extra(&point.payload),
            })
            .collect();
        Ok(Page {
            items,
            next_cursor: page.next_cursor,
        })
    }
}

fn validate_page_limit(limit: u32) -> Result<()> {
    if !(1..=MAX_PAGE_LIMIT).contains(&limit) {
        return Err(KbError::Validation(format!(
            "limit must be between 1 and {MAX_PAGE_LIMIT}, got {limit}"
        )));
    }
    Ok(())
}

pub(super) fn document_summary(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Option<DocumentSummary> {
    let doc_id = payload_str(payload, "doc_id")?;
    if doc_id.trim().is_empty() {
        return None;
    }
    Some(DocumentSummary {
        doc_id: doc_id.to_string(),
        title: payload_str(payload, "title").map(str::to_string),
        source: payload_str(payload, "source").map(str::to_string),
        tags: payload_tags(payload),
        chunk_count: payload_u64(payload, "chunk_count").unwrap_or(1),
        embedding_model: payload_str(payload, "embedding_model").map(str::to_string),
        created_at: payload_str(payload, "created_at").map(str::to_string),
    })
}

pub(super) async fn rerank_hits(
    query: &str,
    reranker: &dyn RerankProvider,
    hits: Vec<SearchHit>,
    top_k: usize,
) -> Result<Vec<SearchHit>> {
    // Empty chunks carry no signal for the reranker and are skipped.
    let candidates: Vec<usize> = hits
        .iter()
        .enumerate()
        .filter_map(|(index, hit)| (!hit.text.trim().is_empty()).then_some(index))
        .collect();
    if candidates.is_empty() {
        // Native chunks always have text. External collections may contain
        // payload-only points, for which preserving vector recall is the only
        // meaningful behavior when a reranker is enabled.
        return Ok(hits.into_iter().take(top_k).collect());
    }

    let texts: Vec<&str> = candidates
        .iter()
        .map(|index| hits[*index].text.as_str())
        .collect();
    let ranked = reranker.rerank(query, &texts).await?;
    if ranked.len() != candidates.len() {
        return Err(KbError::Rerank(format!(
            "reranker returned {} ranks for {} candidates",
            ranked.len(),
            candidates.len()
        )));
    }
    let mut normalized = Vec::new();
    let mut seen = HashSet::new();
    for RankedDocument { index, score } in ranked {
        if index >= candidates.len() {
            return Err(KbError::Rerank(format!(
                "reranker returned out-of-range index {index} for {} candidates",
                candidates.len()
            )));
        }
        if !score.is_finite() {
            return Err(KbError::Rerank(format!(
                "reranker returned a non-finite score for index {index}"
            )));
        }
        if !seen.insert(index) {
            return Err(KbError::Rerank(format!(
                "reranker returned duplicate index {index}"
            )));
        }
        let vector_index = candidates[index];
        normalized.push((index, vector_index, score));
    }
    normalized.sort_by(|left, right| {
        right
            .2
            .total_cmp(&left.2)
            .then_with(|| left.1.cmp(&right.1))
    });

    let mut output = Vec::with_capacity(normalized.len().min(top_k));
    for (_, vector_index, score) in normalized {
        let mut hit = hits[vector_index].clone();
        hit.rerank_score = Some(score);
        output.push(hit);
        if output.len() == top_k {
            return Ok(output);
        }
    }
    Ok(output)
}
