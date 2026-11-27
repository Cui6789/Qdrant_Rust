//! Validation, Qdrant filters and payload mapping shared by service modules.

use qdrant_client::qdrant::r#match::MatchValue;
use qdrant_client::qdrant::{Condition, Filter, ScoredPoint};

use crate::error::{KbError, Result};
use crate::ingest::chunker::Chunk;
use crate::models::{DocumentMetadata, SearchFilter, SearchHit, SearchHitMetadata, SearchRequest};

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

/// Validates a document id: 1-128 characters, non-blank, no control characters.
pub(super) fn validate_doc_id(doc_id: &str) -> Result<()> {
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
pub(super) fn build_filter(filter: Option<&SearchFilter>) -> Option<Filter> {
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

/// Builds the full point payload described by the design.
pub(super) fn payload_for(
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
pub(super) fn metadata_payload(
    metadata: &DocumentMetadata,
) -> serde_json::Map<String, serde_json::Value> {
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
pub(super) fn metadata_matches(
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

/// Compares a stored payload's chunk text and heading path with fresh chunks.
pub(super) fn chunk_payload_matches(
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

/// Extracts search-hit metadata from a stored payload without panicking.
pub(super) fn metadata_from_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> SearchHitMetadata {
    SearchHitMetadata {
        source: payload_str(payload, "source").map(str::to_string),
        title: payload_str(payload, "title").map(str::to_string),
        tags: payload_tags(payload),
        heading_path: payload_str(payload, "heading_path")
            .unwrap_or_default()
            .to_string(),
        extra: payload_extra(payload),
    }
}

/// Maps a scored point into a [`SearchHit`], falling back to empty values.
pub(super) fn hit_from_scored_point(point: ScoredPoint) -> SearchHit {
    let payload = qdrant_payload_to_map(point.payload);
    SearchHit {
        score: point.score,
        rerank_score: None,
        doc_id: payload_str(&payload, "doc_id")
            .unwrap_or_default()
            .to_string(),
        chunk_index: payload_u64(&payload, "chunk_index").unwrap_or(0),
        text: payload_str(&payload, "text")
            .unwrap_or_default()
            .to_string(),
        metadata: metadata_from_payload(&payload),
    }
}

/// Converts a Qdrant payload into a JSON object.
pub(super) fn qdrant_payload_to_map(
    payload: std::collections::HashMap<String, qdrant_client::qdrant::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    payload
        .into_iter()
        .map(|(key, value)| (key, serde_json::Value::from(value)))
        .collect()
}

/// Returns one payload string value.
pub(super) fn payload_str<'a>(
    payload: &'a serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<&'a str> {
    payload.get(key).and_then(serde_json::Value::as_str)
}

/// Returns one payload unsigned integer value.
pub(super) fn payload_u64(
    payload: &serde_json::Map<String, serde_json::Value>,
    key: &str,
) -> Option<u64> {
    payload.get(key).and_then(serde_json::Value::as_u64)
}

/// Returns a payload object, treating a missing field as empty.
pub(super) fn payload_extra(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> serde_json::Map<String, serde_json::Value> {
    payload
        .get("extra")
        .and_then(serde_json::Value::as_object)
        .cloned()
        .unwrap_or_default()
}

/// Returns the tags stored in a payload, treating a missing field as empty.
pub(super) fn payload_tags(payload: &serde_json::Map<String, serde_json::Value>) -> Vec<String> {
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
pub(super) fn identity_condition(field: &str, value: &str) -> Condition {
    Condition::matches(field, MatchValue::Keyword(value.to_string()))
}

/// Filter selecting every point of one document.
pub(super) fn doc_filter(doc_id: &str) -> Filter {
    Filter::must([identity_condition("doc_id", doc_id)])
}

/// Filter selecting one document version by content hash.
pub(super) fn hash_filter(doc_id: &str, content_hash: &str) -> Filter {
    Filter::must([
        identity_condition("doc_id", doc_id),
        identity_condition("content_hash", content_hash),
    ])
}

/// Converts a JSON payload object into a Qdrant payload.
pub(super) fn to_qdrant_payload(
    payload: &serde_json::Map<String, serde_json::Value>,
) -> Result<qdrant_client::Payload> {
    qdrant_client::Payload::try_from(serde_json::Value::Object(payload.clone()))
        .map_err(|error| KbError::Qdrant(format!("failed to encode point payload: {error}")))
}

fn string_array(values: &[String]) -> serde_json::Value {
    serde_json::Value::Array(
        values
            .iter()
            .cloned()
            .map(serde_json::Value::String)
            .collect(),
    )
}

fn optional_string(value: Option<&str>) -> serde_json::Value {
    value.map_or(serde_json::Value::Null, |value| {
        serde_json::Value::String(value.to_string())
    })
}
