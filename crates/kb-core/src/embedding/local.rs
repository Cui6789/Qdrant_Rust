//! In-process embedding provider backed by fastembed (ONNX Runtime).

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastembed::{EmbeddingModel, TextEmbedding, TextInitOptions};

use super::EmbeddingProvider;
use crate::config::LocalEmbeddingConfig;
use crate::error::{KbError, Result};

/// Query instruction recommended for the BGE Chinese v1.5 models.
const BGE_ZH_QUERY_PREFIX: &str = "为这个句子生成表示以用于检索相关文章：";

/// Friendly model aliases accepted by [`model_from_alias`], in display order.
const SUPPORTED_ALIASES: &[(&str, EmbeddingModel)] = &[
    ("bge-small-zh-v1.5", EmbeddingModel::BGESmallZHV15),
    ("bge-large-zh-v1.5", EmbeddingModel::BGELargeZHV15),
    ("bge-m3", EmbeddingModel::BGEM3),
    ("multilingual-e5-small", EmbeddingModel::MultilingualE5Small),
    ("multilingual-e5-base", EmbeddingModel::MultilingualE5Base),
    ("multilingual-e5-large", EmbeddingModel::MultilingualE5Large),
    ("bge-small-en-v1.5", EmbeddingModel::BGESmallENV15),
    ("all-minilm-l6-v2", EmbeddingModel::AllMiniLML6V2),
    ("nomic-embed-text-v1.5", EmbeddingModel::NomicEmbedTextV15),
];

/// Resolves a friendly model alias (case-insensitive, trimmed) to a fastembed model.
///
/// Returns [`KbError::Validation`] listing the supported aliases for anything else.
pub fn model_from_alias(alias: &str) -> Result<EmbeddingModel> {
    let normalized = alias.trim().to_ascii_lowercase();
    SUPPORTED_ALIASES
        .iter()
        .find(|(name, _)| *name == normalized.as_str())
        .map(|(_, model)| model.clone())
        .ok_or_else(|| {
            let supported = SUPPORTED_ALIASES
                .iter()
                .map(|(name, _)| *name)
                .collect::<Vec<_>>()
                .join(", ");
            KbError::Validation(format!(
                "unsupported local embedding model alias `{alias}`; supported aliases: {supported}"
            ))
        })
}

/// Returns the default `(query, document)` prefixes for a model alias.
///
/// Prefixes follow each model card; models without a prefix convention, and
/// unknown aliases, return two empty strings. fastembed itself adds none.
pub fn default_prefixes(alias: &str) -> (String, String) {
    match alias.trim().to_ascii_lowercase().as_str() {
        "bge-small-zh-v1.5" | "bge-large-zh-v1.5" => {
            (BGE_ZH_QUERY_PREFIX.to_string(), String::new())
        }
        "multilingual-e5-small" | "multilingual-e5-base" | "multilingual-e5-large" => {
            ("query: ".to_string(), "passage: ".to_string())
        }
        "nomic-embed-text-v1.5" => (
            "search_query: ".to_string(),
            "search_document: ".to_string(),
        ),
        _ => (String::new(), String::new()),
    }
}

/// Resolves `(query, document)` prefixes, preferring non-empty configured values.
pub fn resolve_prefixes(config: &LocalEmbeddingConfig, alias: &str) -> (String, String) {
    let (default_query, default_document) = default_prefixes(alias);
    let query = if config.query_prefix.is_empty() {
        default_query
    } else {
        config.query_prefix.clone()
    };
    let document = if config.document_prefix.is_empty() {
        default_document
    } else {
        config.document_prefix.clone()
    };
    (query, document)
}

/// Everything needed to load the ONNX model on first use or during warmup.
#[derive(Clone)]
struct LocalInit {
    model: EmbeddingModel,
    cache_dir: PathBuf,
    show_download_progress: bool,
    max_length: usize,
}

impl LocalInit {
    /// Loads the model, downloading it into the cache directory when missing.
    fn load(&self) -> Result<TextEmbedding> {
        TextEmbedding::try_new(
            TextInitOptions::new(self.model.clone())
                .with_cache_dir(self.cache_dir.clone())
                .with_show_download_progress(self.show_download_progress)
                .with_max_length(self.max_length),
        )
        .map_err(|error| {
            KbError::Embedding(format!("failed to load local embedding model: {error}"))
        })
    }
}

/// Embedding provider backed by a local fastembed (ONNX Runtime) model.
///
/// The model may be loaded during construction (`warmup = true`) or lazily on
/// the first embedding call. Inference always runs on a blocking thread so the
/// async runtime is never blocked.
pub struct LocalEmbedding {
    model_alias: String,
    dimension: usize,
    batch_size: usize,
    query_prefix: String,
    document_prefix: String,
    model: Arc<Mutex<Option<TextEmbedding>>>,
    init: LocalInit,
}

impl LocalEmbedding {
    /// Builds a provider for `config`.
    ///
    /// The dimension is read from fastembed's model metadata without loading
    /// the model; the ONNX model itself is loaded eagerly only when
    /// `config.warmup` is set.
    pub fn new(config: &LocalEmbeddingConfig, batch_size: usize) -> Result<Self> {
        let model = model_from_alias(&config.model)?;
        let dimension = TextEmbedding::get_model_info(&model)
            .map_err(|error| {
                KbError::Embedding(format!("failed to look up embedding model info: {error}"))
            })?
            .dim;
        let (query_prefix, document_prefix) = resolve_prefixes(config, &config.model);
        let init = LocalInit {
            model,
            cache_dir: PathBuf::from(&config.cache_dir),
            show_download_progress: config.show_download_progress,
            max_length: config.max_length,
        };
        let model = if config.warmup {
            Some(init.load()?)
        } else {
            None
        };

        Ok(Self {
            model_alias: config.model.clone(),
            dimension,
            batch_size: batch_size.max(1),
            query_prefix,
            document_prefix,
            model: Arc::new(Mutex::new(model)),
            init,
        })
    }

    /// Prefixes `texts` and embeds them on a blocking thread.
    async fn embed_with_prefix(&self, texts: &[String], prefix: &str) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let prefixed: Vec<String> = if prefix.is_empty() {
            texts.to_vec()
        } else {
            texts.iter().map(|text| format!("{prefix}{text}")).collect()
        };
        let model = Arc::clone(&self.model);
        let init = self.init.clone();
        let batch_size = self.batch_size;

        tokio::task::spawn_blocking(move || -> Result<Vec<Vec<f32>>> {
            let mut guard = model.lock().map_err(|_| {
                KbError::Embedding("local embedding model lock is poisoned".to_string())
            })?;
            if guard.is_none() {
                *guard = Some(init.load()?);
            }
            let model = guard.as_mut().expect("model loaded above");
            model
                .embed(prefixed, Some(batch_size))
                .map_err(|error| KbError::Embedding(format!("failed to embed texts: {error}")))
        })
        .await
        .map_err(|error| KbError::Embedding(format!("local embedding task failed: {error}")))?
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for LocalEmbedding {
    fn model_id(&self) -> &str {
        &self.model_alias
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        self.embed_with_prefix(texts, &self.document_prefix).await
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let vectors = self
            .embed_with_prefix(&[text.to_string()], &self.query_prefix)
            .await?;
        vectors.into_iter().next().ok_or_else(|| {
            KbError::Embedding("local embedding returned no vector for the query".to_string())
        })
    }
}

#[cfg(test)]
mod tests {
    use fastembed::EmbeddingModel;

    use super::*;
    use crate::config::LocalEmbeddingConfig;
    use crate::embedding::EmbeddingProvider;
    use crate::error::KbError;

    #[test]
    fn maps_known_aliases() {
        assert_eq!(
            model_from_alias("bge-small-zh-v1.5").unwrap(),
            EmbeddingModel::BGESmallZHV15
        );
        assert_eq!(
            model_from_alias("bge-large-zh-v1.5").unwrap(),
            EmbeddingModel::BGELargeZHV15
        );
        assert_eq!(model_from_alias("bge-m3").unwrap(), EmbeddingModel::BGEM3);
        assert_eq!(
            model_from_alias("multilingual-e5-small").unwrap(),
            EmbeddingModel::MultilingualE5Small
        );
        assert_eq!(
            model_from_alias("multilingual-e5-base").unwrap(),
            EmbeddingModel::MultilingualE5Base
        );
        assert_eq!(
            model_from_alias("multilingual-e5-large").unwrap(),
            EmbeddingModel::MultilingualE5Large
        );
        assert_eq!(
            model_from_alias("bge-small-en-v1.5").unwrap(),
            EmbeddingModel::BGESmallENV15
        );
        assert_eq!(
            model_from_alias("all-minilm-l6-v2").unwrap(),
            EmbeddingModel::AllMiniLML6V2
        );
        assert_eq!(
            model_from_alias("nomic-embed-text-v1.5").unwrap(),
            EmbeddingModel::NomicEmbedTextV15
        );
    }

    #[test]
    fn unknown_alias_lists_supported_aliases() {
        let error = model_from_alias("not-a-model").unwrap_err();
        let KbError::Validation(message) = error else {
            panic!("expected a validation error, got {error:?}");
        };
        for alias in [
            "bge-small-zh-v1.5",
            "bge-large-zh-v1.5",
            "bge-m3",
            "multilingual-e5-small",
            "multilingual-e5-base",
            "multilingual-e5-large",
            "bge-small-en-v1.5",
            "all-minilm-l6-v2",
            "nomic-embed-text-v1.5",
        ] {
            assert!(message.contains(alias), "missing {alias} in: {message}");
        }
    }

    #[test]
    fn prefix_defaults_match_model_cards() {
        assert_eq!(
            default_prefixes("bge-small-zh-v1.5"),
            (
                "为这个句子生成表示以用于检索相关文章：".to_string(),
                String::new()
            )
        );
        assert_eq!(
            default_prefixes("bge-large-zh-v1.5"),
            (
                "为这个句子生成表示以用于检索相关文章：".to_string(),
                String::new()
            )
        );
        for alias in [
            "multilingual-e5-small",
            "multilingual-e5-base",
            "multilingual-e5-large",
        ] {
            assert_eq!(
                default_prefixes(alias),
                ("query: ".to_string(), "passage: ".to_string())
            );
        }
        assert_eq!(
            default_prefixes("nomic-embed-text-v1.5"),
            (
                "search_query: ".to_string(),
                "search_document: ".to_string()
            )
        );
        assert_eq!(default_prefixes("bge-m3"), (String::new(), String::new()));
        assert_eq!(
            default_prefixes("bge-small-en-v1.5"),
            (String::new(), String::new())
        );
    }

    #[test]
    fn config_prefix_overrides_default() {
        let mut config = LocalEmbeddingConfig {
            model: "bge-small-zh-v1.5".to_string(),
            query_prefix: "custom query: ".to_string(),
            document_prefix: "custom document: ".to_string(),
            ..LocalEmbeddingConfig::default()
        };

        assert_eq!(
            resolve_prefixes(&config, "bge-small-zh-v1.5"),
            (
                "custom query: ".to_string(),
                "custom document: ".to_string()
            )
        );

        config.query_prefix.clear();
        assert_eq!(
            resolve_prefixes(&config, "bge-small-zh-v1.5"),
            (
                "为这个句子生成表示以用于检索相关文章：".to_string(),
                "custom document: ".to_string()
            )
        );

        config.document_prefix.clear();
        assert_eq!(
            resolve_prefixes(&config, "bge-small-zh-v1.5"),
            (
                "为这个句子生成表示以用于检索相关文章：".to_string(),
                String::new()
            )
        );
    }

    #[tokio::test]
    #[ignore = "downloads the bge-small-zh-v1.5 model from Hugging Face"]
    async fn embeds_chinese_text_with_bge_small_zh() {
        let cache_dir = std::env::temp_dir().join(format!("kb-fastembed-{}", uuid::Uuid::new_v4()));
        let config = LocalEmbeddingConfig {
            model: "bge-small-zh-v1.5".to_string(),
            cache_dir: cache_dir.to_string_lossy().into_owned(),
            show_download_progress: false,
            max_length: 512,
            warmup: false,
            ..LocalEmbeddingConfig::default()
        };

        let provider = LocalEmbedding::new(&config, 8).unwrap();
        assert_eq!(provider.dimension(), 512);

        let vectors = provider
            .embed_documents(&["今天天气怎么样".to_string(), "年假怎么申请".to_string()])
            .await
            .unwrap();
        assert_eq!(vectors.len(), 2);
        for vector in &vectors {
            assert_eq!(vector.len(), 512);
            let norm = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
            assert!((norm - 1.0).abs() < 1e-3, "L2 norm was {norm}");
        }

        let _ = std::fs::remove_dir_all(&cache_dir);
    }
}
