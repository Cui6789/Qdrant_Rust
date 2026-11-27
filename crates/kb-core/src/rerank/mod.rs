//! Optional retrieval-result reranking.
//!
//! Reranking is disabled by default. A caller composes
//! [`build_reranker`] into its retrieval pipeline when it wants either the
//! in-process fastembed provider or an HTTP provider.

mod api;
#[cfg(feature = "local-rerank")]
mod local;

use std::fmt;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::error::{KbError, Result};

/// Default number of retrieval candidates passed to reranking by callers.
pub const DEFAULT_CANDIDATE_LIMIT: u64 = 30;
/// Maximum number of retrieval candidates passed to reranking by callers.
pub const MAX_CANDIDATE_LIMIT: u64 = 100;
/// Default multilingual fastembed reranker.
pub const DEFAULT_LOCAL_RERANK_MODEL: &str = "bge-reranker-v2-m3";
/// Default fastembed cache shared with the local embedding provider.
pub const DEFAULT_LOCAL_RERANK_CACHE_DIR: &str = "./data/models";
/// Default SiliconFlow rerank endpoint.
pub const DEFAULT_API_RERANK_ENDPOINT: &str = "https://api.siliconflow.com/v1/rerank";
/// Model used by SiliconFlow's documented rerank example.
pub const DEFAULT_API_RERANK_MODEL: &str = "Qwen/Qwen3-Reranker-8B";
/// Default HTTP request timeout.
pub const DEFAULT_API_RERANK_TIMEOUT_SECS: u64 = 60;

const SUPPORTED_LOCAL_MODELS: &[(&str, &str)] = &[
    ("bge-reranker-base", "BAAI/bge-reranker-base"),
    ("bge-reranker-v2-m3", "rozgo/bge-reranker-v2-m3"),
    (
        "jina-reranker-v1-turbo-en",
        "jinaai/jina-reranker-v1-turbo-en",
    ),
    (
        "jina-reranker-v2-base-multilingual",
        "jinaai/jina-reranker-v2-base-multilingual",
    ),
];

/// Provider selection and shared rerank settings.
///
/// `provider` is `disabled`, `local`, or `api`. The string field preserves
/// compatibility with configurations that predate reranking. Reranking is
/// disabled when the field is absent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct RerankConfig {
    pub provider: String,
    pub candidate_limit: u64,
    pub local: LocalRerankConfig,
    pub api: ApiRerankConfig,
}

impl Default for RerankConfig {
    fn default() -> Self {
        Self {
            provider: "disabled".to_string(),
            candidate_limit: DEFAULT_CANDIDATE_LIMIT,
            local: LocalRerankConfig::default(),
            api: ApiRerankConfig::default(),
        }
    }
}

/// In-process fastembed reranker settings.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalRerankConfig {
    pub model: String,
    pub cache_dir: String,
}

impl Default for LocalRerankConfig {
    fn default() -> Self {
        Self {
            model: DEFAULT_LOCAL_RERANK_MODEL.to_string(),
            cache_dir: DEFAULT_LOCAL_RERANK_CACHE_DIR.to_string(),
        }
    }
}

/// SiliconFlow/Cohere-compatible HTTP reranker settings.
///
/// `endpoint` is the complete POST URL, including any API version and
/// `/rerank` path. An empty bearer key is allowed for local-compatible
/// servers; otherwise it is sent as a bearer token.
#[derive(Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiRerankConfig {
    pub endpoint: String,
    pub model: String,
    pub api_key: String,
    pub timeout_secs: u64,
}

impl Default for ApiRerankConfig {
    fn default() -> Self {
        Self {
            endpoint: DEFAULT_API_RERANK_ENDPOINT.to_string(),
            model: DEFAULT_API_RERANK_MODEL.to_string(),
            api_key: String::new(),
            timeout_secs: DEFAULT_API_RERANK_TIMEOUT_SECS,
        }
    }
}

impl fmt::Debug for ApiRerankConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        let endpoint = if self.endpoint.trim().is_empty() {
            "<unset>"
        } else {
            "<configured>"
        };
        let api_key = if self.api_key.trim().is_empty() {
            "<unset>"
        } else {
            "<redacted>"
        };

        formatter
            .debug_struct("ApiRerankConfig")
            .field("endpoint", &endpoint)
            .field("model", &self.model)
            .field("api_key", &api_key)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// One document index and its rerank score.
#[derive(Debug, Clone, PartialEq)]
pub struct RankedDocument {
    pub index: usize,
    pub score: f32,
}

/// A pluggable query/document reranking provider.
#[async_trait::async_trait]
pub trait RerankProvider: Send + Sync {
    /// Scores every input document and returns a descending-score permutation.
    async fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RankedDocument>>;
}

/// Builds the configured reranker.
///
/// `None` is returned only for the explicitly disabled provider. Invalid
/// active-provider settings and unavailable features produce errors; there is
/// no fallback to another provider.
pub async fn build_reranker(config: &RerankConfig) -> Result<Option<Arc<dyn RerankProvider>>> {
    validate_config(config)?;

    match provider_kind(&config.provider)? {
        ProviderKind::Disabled => Ok(None),
        ProviderKind::Local => build_local(config.local.clone()).await.map(Some),
        ProviderKind::Api => {
            let provider = api::ApiRerank::new(&config.api)?;
            Ok(Some(Arc::new(provider)))
        }
    }
}

/// Validates rerank settings for startup, administration, and hot reload.
pub fn validate_config(config: &RerankConfig) -> Result<()> {
    let provider = provider_kind(&config.provider)?;
    if !(1..=MAX_CANDIDATE_LIMIT).contains(&config.candidate_limit) {
        return Err(KbError::Config(format!(
            "rerank.candidate_limit must be between 1 and {MAX_CANDIDATE_LIMIT}"
        )));
    }

    match provider {
        ProviderKind::Disabled => Ok(()),
        ProviderKind::Local => validate_local_config(&config.local),
        ProviderKind::Api => validate_api_config(&config.api),
    }
}

#[cfg(feature = "local-rerank")]
async fn build_local(config: LocalRerankConfig) -> Result<Arc<dyn RerankProvider>> {
    let provider = tokio::task::spawn_blocking(move || local::LocalRerank::new(&config))
        .await
        .map_err(|error| {
            KbError::Rerank(format!("local rerank initialization task failed: {error}"))
        })??;
    Ok(Arc::new(provider))
}

#[cfg(not(feature = "local-rerank"))]
async fn build_local(_config: LocalRerankConfig) -> Result<Arc<dyn RerankProvider>> {
    Err(KbError::Config(
        "local-rerank feature is not enabled".to_string(),
    ))
}

#[derive(Clone, Copy)]
enum ProviderKind {
    Disabled,
    Local,
    Api,
}

fn provider_kind(value: &str) -> Result<ProviderKind> {
    match value.trim().to_ascii_lowercase().as_str() {
        "disabled" => Ok(ProviderKind::Disabled),
        "local" => Ok(ProviderKind::Local),
        "api" => Ok(ProviderKind::Api),
        _ => Err(KbError::Config(
            "rerank.provider must be one of `disabled`, `local`, or `api`".to_string(),
        )),
    }
}

fn validate_local_config(config: &LocalRerankConfig) -> Result<()> {
    if config.model.trim().is_empty() {
        return Err(KbError::Config(
            "rerank.local.model must not be empty when provider is local".to_string(),
        ));
    }
    if config.cache_dir.trim().is_empty() {
        return Err(KbError::Config(
            "rerank.local.cache_dir must not be empty when provider is local".to_string(),
        ));
    }
    if local_model_code(&config.model).is_none() {
        return Err(KbError::Config(
            "rerank.local.model is not supported by the bundled fastembed rerankers; \
             use bge-reranker-base, bge-reranker-v2-m3, \
             jina-reranker-v1-turbo-en, or jina-reranker-v2-base-multilingual"
                .to_string(),
        ));
    }
    Ok(())
}

fn validate_api_config(config: &ApiRerankConfig) -> Result<()> {
    let endpoint = config.endpoint.trim();
    if endpoint.is_empty() {
        return Err(KbError::Config(
            "rerank.api.endpoint must not be empty when provider is api".to_string(),
        ));
    }
    let parsed = reqwest::Url::parse(endpoint).map_err(|_| {
        KbError::Config("rerank.api.endpoint must be a valid HTTP or HTTPS URL".to_string())
    })?;
    if !matches!(parsed.scheme(), "http" | "https") || !parsed.has_host() {
        return Err(KbError::Config(
            "rerank.api.endpoint must be an absolute HTTP or HTTPS URL".to_string(),
        ));
    }
    if !parsed.username().is_empty() || parsed.password().is_some() {
        return Err(KbError::Config(
            "rerank.api.endpoint must not contain URL userinfo".to_string(),
        ));
    }
    if parsed.query().is_some() {
        return Err(KbError::Config(
            "rerank.api.endpoint must not contain a query string".to_string(),
        ));
    }
    if parsed.fragment().is_some() {
        return Err(KbError::Config(
            "rerank.api.endpoint must not contain a URL fragment".to_string(),
        ));
    }
    if config.model.trim().is_empty() {
        return Err(KbError::Config(
            "rerank.api.model must not be empty when provider is api".to_string(),
        ));
    }
    if config
        .api_key
        .trim()
        .chars()
        .any(|character| character.is_control())
    {
        return Err(KbError::Config(
            "rerank.api.api_key must not contain control characters".to_string(),
        ));
    }
    if config.timeout_secs == 0 {
        return Err(KbError::Config(
            "rerank.api.timeout_secs must be greater than 0 when provider is api".to_string(),
        ));
    }
    Ok(())
}

fn local_model_code(model: &str) -> Option<&'static str> {
    let normalized = model.trim().to_ascii_lowercase();
    if let Some((_, code)) = SUPPORTED_LOCAL_MODELS
        .iter()
        .find(|(alias, _)| *alias == normalized.as_str())
    {
        return Some(*code);
    }

    SUPPORTED_LOCAL_MODELS
        .iter()
        .find(|(_, code)| code.eq_ignore_ascii_case(&normalized))
        .map(|(_, code)| *code)
}

fn validate_ranked_documents(
    expected_count: usize,
    mut ranked: Vec<RankedDocument>,
) -> Result<Vec<RankedDocument>> {
    if ranked.len() != expected_count {
        return Err(KbError::Rerank(format!(
            "rerank provider returned {} results for {expected_count} documents",
            ranked.len()
        )));
    }

    let mut seen = vec![false; expected_count];
    for document in &ranked {
        if document.index >= expected_count {
            return Err(KbError::Rerank(format!(
                "rerank provider returned out-of-range document index {} for {expected_count} documents",
                document.index
            )));
        }
        if seen[document.index] {
            return Err(KbError::Rerank(format!(
                "rerank provider returned duplicate document index {}",
                document.index
            )));
        }
        if !document.score.is_finite() {
            return Err(KbError::Rerank(format!(
                "rerank provider returned a non-finite score for document index {}",
                document.index
            )));
        }
        seen[document.index] = true;
    }

    ranked.sort_by(|left, right| {
        right
            .score
            .total_cmp(&left.score)
            .then_with(|| left.index.cmp(&right.index))
    });
    Ok(ranked)
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn defaults_disable_reranking_for_legacy_configs() {
        let config = RerankConfig::default();
        assert_eq!(config.provider, "disabled");
        assert_eq!(config.candidate_limit, 30);
        assert_eq!(config.local.model, "bge-reranker-v2-m3");
        assert_eq!(config.local.cache_dir, "./data/models");
        assert_eq!(config.api.endpoint, "https://api.siliconflow.com/v1/rerank");
        assert_eq!(config.api.model, "Qwen/Qwen3-Reranker-8B");
        assert!(config.api.api_key.is_empty());
        assert_eq!(config.api.timeout_secs, 60);

        let legacy: RerankConfig = serde_json::from_value(json!({})).unwrap();
        assert_eq!(legacy, config);
    }

    #[test]
    fn config_equality_and_debug_do_not_expose_credentials_or_endpoint() {
        let mut config = RerankConfig {
            provider: "api".to_string(),
            ..RerankConfig::default()
        };
        config.api.endpoint = "https://private.example.test/path-with-token/v1/rerank".to_string();
        config.api.api_key = "sk-super-secret".to_string();

        assert_eq!(config.clone(), config);
        let debug = format!("{config:?}");
        assert!(debug.contains("<redacted>"));
        assert!(debug.contains("<configured>"));
        assert!(!debug.contains("sk-super-secret"));
        assert!(!debug.contains("private.example.test"));
        assert!(!debug.contains("path-with-token"));
    }

    #[test]
    fn validates_api_endpoint_without_echoing_it() {
        for endpoint in [
            "https://user:password@example.com/v1/rerank",
            "https://example.com/v1/rerank?token=secret",
            "https://example.com/v1/rerank#secret",
            "ftp://example.com/v1/rerank",
            "not a URL",
        ] {
            let mut config = RerankConfig {
                provider: "api".to_string(),
                ..RerankConfig::default()
            };
            config.api.endpoint = endpoint.to_string();

            let error = validate_config(&config).unwrap_err();
            assert!(matches!(error, KbError::Config(_)));
            let message = error.to_string();
            assert!(!message.contains(endpoint));
            assert!(!message.contains("password"));
            assert!(!message.contains("token=secret"));
        }
    }

    #[test]
    fn empty_api_key_is_valid_for_compatible_endpoints() {
        let mut config = RerankConfig {
            provider: "api".to_string(),
            ..RerankConfig::default()
        };
        config.api.endpoint = "http://127.0.0.1:8080/rerank".to_string();
        config.api.api_key.clear();
        validate_config(&config).unwrap();
    }

    #[test]
    fn candidate_limit_and_active_provider_settings_are_validated() {
        let mut config = RerankConfig {
            candidate_limit: 0,
            ..RerankConfig::default()
        };
        assert!(matches!(validate_config(&config), Err(KbError::Config(_))));

        config.candidate_limit = 101;
        assert!(matches!(validate_config(&config), Err(KbError::Config(_))));

        config.candidate_limit = 1;
        validate_config(&config).unwrap();
        config.candidate_limit = 100;
        validate_config(&config).unwrap();

        config.candidate_limit = 30;
        config.provider = "local".to_string();
        config.local.model = "not-a-fastembed-reranker".to_string();
        assert!(matches!(validate_config(&config), Err(KbError::Config(_))));

        config.provider = "disabled".to_string();
        validate_config(&config).unwrap();
    }

    #[test]
    fn unknown_provider_has_a_fixed_error() {
        let config = RerankConfig {
            provider: "sk-secret-provider".to_string(),
            ..RerankConfig::default()
        };
        let error = validate_config(&config).unwrap_err();
        let message = error.to_string();
        assert!(message.contains("disabled"));
        assert!(!message.contains("sk-secret-provider"));
    }

    #[tokio::test]
    async fn build_returns_none_only_when_disabled() {
        assert!(build_reranker(&RerankConfig::default())
            .await
            .unwrap()
            .is_none());
    }

    #[test]
    fn result_validation_sorts_and_breaks_ties_by_index() {
        let ranked = validate_ranked_documents(
            3,
            vec![
                RankedDocument {
                    index: 2,
                    score: 0.1,
                },
                RankedDocument {
                    index: 1,
                    score: 0.8,
                },
                RankedDocument {
                    index: 0,
                    score: 0.8,
                },
            ],
        )
        .unwrap();

        assert_eq!(
            ranked,
            vec![
                RankedDocument {
                    index: 0,
                    score: 0.8,
                },
                RankedDocument {
                    index: 1,
                    score: 0.8,
                },
                RankedDocument {
                    index: 2,
                    score: 0.1,
                },
            ]
        );
    }
}
