use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{KbError, Result};

/// Document formats accepted by the ingestion pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DocumentFormat {
    #[default]
    Text,
    Markdown,
    Html,
    Json,
}

impl DocumentFormat {
    /// Infers the format from a filename extension; unknown extensions are text.
    pub fn infer_from_filename(name: &str) -> Self {
        let extension = Path::new(name)
            .extension()
            .and_then(|extension| extension.to_str())
            .unwrap_or_default()
            .to_ascii_lowercase();
        match extension.as_str() {
            "md" | "markdown" => Self::Markdown,
            "html" | "htm" => Self::Html,
            "json" => Self::Json,
            _ => Self::Text,
        }
    }

    /// Parses a lowercase format name, rejecting unknown values.
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "text" => Ok(Self::Text),
            "markdown" => Ok(Self::Markdown),
            "html" => Ok(Self::Html),
            "json" => Ok(Self::Json),
            other => Err(KbError::UnsupportedFormat(format!(
                "unsupported document format: '{other}'"
            ))),
        }
    }
}

/// Caller-supplied document metadata.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct DocumentMetadata {
    pub source: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Request body for text ingestion.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct IngestDocumentRequest {
    #[serde(default)]
    pub doc_id: Option<String>,
    pub text: String,
    #[serde(default)]
    pub format: Option<String>,
    #[serde(default)]
    pub metadata: DocumentMetadata,
}

/// Result returned after ingesting a document.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct IngestResult {
    pub doc_id: String,
    pub chunks: usize,
    pub skipped: bool,
    pub metadata_updated: bool,
    pub content_hash: String,
    pub embedding_model: String,
}

fn default_top_k() -> usize {
    5
}

/// Request body for similarity search.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchRequest {
    pub query: String,
    #[serde(default = "default_top_k")]
    pub top_k: usize,
    #[serde(default)]
    pub score_threshold: Option<f32>,
    #[serde(default)]
    pub filter: Option<SearchFilter>,
}

/// Optional metadata filters for search. Different fields are ANDed together.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchFilter {
    pub doc_ids: Option<Vec<String>>,
    pub tags_any: Option<Vec<String>>,
    pub source: Option<String>,
}

/// Metadata attached to a search hit.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchHitMetadata {
    pub source: Option<String>,
    pub title: Option<String>,
    pub tags: Vec<String>,
    pub heading_path: String,
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// A single similarity search result.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct SearchHit {
    pub score: f32,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rerank_score: Option<f32>,
    pub doc_id: String,
    pub chunk_index: u64,
    pub text: String,
    pub metadata: SearchHitMetadata,
}

/// One page returned by a cursor-based browsing API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Page<T> {
    pub items: Vec<T>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub next_cursor: Option<String>,
}

/// A document-level summary assembled from its first stored chunk.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentSummary {
    pub doc_id: String,
    pub title: Option<String>,
    pub source: Option<String>,
    #[serde(default)]
    pub tags: Vec<String>,
    pub chunk_count: u64,
    pub embedding_model: Option<String>,
    pub created_at: Option<String>,
}

/// A stored chunk returned by the document browsing API.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DocumentChunk {
    pub point_id: String,
    pub chunk_index: u64,
    pub text: String,
    #[serde(default)]
    pub heading_path: String,
    #[serde(default)]
    pub extra: serde_json::Map<String, serde_json::Value>,
}

/// Collection summary returned by the collection endpoints.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CollectionInfo {
    pub name: String,
    pub points_count: u64,
    pub vector_size: u64,
}

/// Health-check payload.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HealthStatus {
    pub status: String,
    pub qdrant: String,
    pub embedding: EmbeddingInfo,
}

/// Embedding provider details reported by the health endpoint.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct EmbeddingInfo {
    pub provider: String,
    pub model: String,
    pub dimension: usize,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::KbError;

    #[test]
    fn infers_format_from_filename() {
        assert_eq!(
            DocumentFormat::infer_from_filename("notes.md"),
            DocumentFormat::Markdown
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("NOTES.MARKDOWN"),
            DocumentFormat::Markdown
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("page.html"),
            DocumentFormat::Html
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("page.htm"),
            DocumentFormat::Html
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("data.json"),
            DocumentFormat::Json
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("readme.txt"),
            DocumentFormat::Text
        );
        assert_eq!(
            DocumentFormat::infer_from_filename("README"),
            DocumentFormat::Text
        );
    }

    #[test]
    fn parses_known_format_names_and_rejects_unknown() {
        assert_eq!(DocumentFormat::parse("text").unwrap(), DocumentFormat::Text);
        assert_eq!(
            DocumentFormat::parse("markdown").unwrap(),
            DocumentFormat::Markdown
        );
        assert_eq!(DocumentFormat::parse("html").unwrap(), DocumentFormat::Html);
        assert_eq!(DocumentFormat::parse("json").unwrap(), DocumentFormat::Json);
        for unknown in ["pdf", "", "Markdown", "TEXT"] {
            assert!(matches!(
                DocumentFormat::parse(unknown),
                Err(KbError::UnsupportedFormat(_))
            ));
        }
    }

    #[test]
    fn search_request_defaults_optional_fields() {
        let req: SearchRequest = serde_json::from_str(r#"{"query": "年假怎么申请"}"#).unwrap();
        assert_eq!(req.query, "年假怎么申请");
        assert_eq!(req.top_k, 5);
        assert!(req.score_threshold.is_none());
        assert!(req.filter.is_none());
    }

    #[test]
    fn omits_absent_rerank_score_and_cursor() {
        let hit = SearchHit {
            score: 0.75,
            rerank_score: None,
            doc_id: "doc".to_string(),
            chunk_index: 0,
            text: "text".to_string(),
            metadata: SearchHitMetadata::default(),
        };
        let value = serde_json::to_value(hit).unwrap();
        assert!(value.get("rerank_score").is_none());

        let page = Page {
            items: vec![1_u8],
            next_cursor: None,
        };
        let value = serde_json::to_value(page).unwrap();
        assert!(value.get("next_cursor").is_none());
    }
}
