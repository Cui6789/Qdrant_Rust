use std::path::{Path, PathBuf};

use figment::{
    providers::{Env, Format, Serialized, Toml},
    Figment,
};
use serde::{Deserialize, Serialize};

use crate::error::{KbError, Result};

/// Default location of the configuration file, relative to the process cwd.
const DEFAULT_CONFIG_PATH: &str = "config/config.toml";

/// Fully resolved application configuration.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct AppConfig {
    pub server: ServerConfig,
    pub qdrant: QdrantConfig,
    pub embedding: EmbeddingConfig,
    pub ingest: IngestConfig,
}

impl AppConfig {
    /// Loads the configuration from defaults, an optional TOML file, and `KB_`
    /// environment variables.
    ///
    /// When `path` is given (or `KB_CONFIG` is set), that file must exist.
    /// Otherwise `config/config.toml` is merged only when present.
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let env_path = std::env::var("KB_CONFIG")
            .ok()
            .filter(|value| !value.trim().is_empty());
        let explicit_path: Option<PathBuf> = match path {
            Some(path) => Some(path.to_path_buf()),
            None => env_path.map(PathBuf::from),
        };

        let mut figment = Figment::from(Serialized::defaults(AppConfig::default()));
        match explicit_path {
            Some(path) => figment = figment.merge(Toml::file_exact(path)),
            None => {
                let default_path = Path::new(DEFAULT_CONFIG_PATH);
                if default_path.exists() {
                    figment = figment.merge(Toml::file(default_path));
                }
            }
        }

        let mut config: AppConfig = figment
            .merge(Env::prefixed("KB_").split("__"))
            .extract()
            .map_err(|err| KbError::Config(format!("failed to load configuration: {err}")))?;
        config.trim();
        config.validate()?;
        Ok(config)
    }

    /// Validates cross-field invariants; values are expected to be trimmed.
    pub fn validate(&self) -> Result<()> {
        if self.server.port == 0 {
            return Err(KbError::Config("server.port must be greater than 0".into()));
        }
        if self.server.max_body_mb == 0 {
            return Err(KbError::Config(
                "server.max_body_mb must be greater than 0".into(),
            ));
        }
        if self.server.request_timeout_secs == 0 {
            return Err(KbError::Config(
                "server.request_timeout_secs must be greater than 0".into(),
            ));
        }
        if self.qdrant.url.is_empty() {
            return Err(KbError::Config("qdrant.url must not be empty".into()));
        }
        if self.qdrant.timeout_secs == 0 {
            return Err(KbError::Config(
                "qdrant.timeout_secs must be greater than 0".into(),
            ));
        }
        if self.qdrant.startup_retry_secs == 0 {
            return Err(KbError::Config(
                "qdrant.startup_retry_secs must be greater than 0".into(),
            ));
        }
        if self.embedding.batch_size == 0 {
            return Err(KbError::Config(
                "embedding.batch_size must be greater than 0".into(),
            ));
        }
        if self.embedding.max_concurrent_batches == 0 {
            return Err(KbError::Config(
                "embedding.max_concurrent_batches must be greater than 0".into(),
            ));
        }
        if self.embedding.api.timeout_secs == 0 {
            return Err(KbError::Config(
                "embedding.api.timeout_secs must be greater than 0".into(),
            ));
        }
        match self.embedding.provider {
            ProviderKind::Local if self.embedding.local.model.is_empty() => {
                return Err(KbError::Config(
                    "embedding.local.model must not be empty when provider is local".into(),
                ));
            }
            ProviderKind::Api => {
                if self.embedding.api.base_url.is_empty() {
                    return Err(KbError::Config(
                        "embedding.api.base_url must not be empty when provider is api".into(),
                    ));
                }
                if self.embedding.api.model.is_empty() {
                    return Err(KbError::Config(
                        "embedding.api.model must not be empty when provider is api".into(),
                    ));
                }
            }
            _ => {}
        }
        if self.ingest.chunk_size == 0 {
            return Err(KbError::Config(
                "ingest.chunk_size must be greater than 0".into(),
            ));
        }
        if self.ingest.chunk_overlap >= self.ingest.chunk_size {
            return Err(KbError::Config(
                "ingest.chunk_overlap must be smaller than ingest.chunk_size".into(),
            ));
        }
        if self.ingest.min_chunk_size > self.ingest.chunk_size {
            return Err(KbError::Config(
                "ingest.min_chunk_size must not exceed ingest.chunk_size".into(),
            ));
        }
        if self.ingest.upsert_batch_size == 0 {
            return Err(KbError::Config(
                "ingest.upsert_batch_size must be greater than 0".into(),
            ));
        }
        Ok(())
    }

    fn trim(&mut self) {
        self.server.host = self.server.host.trim().to_string();
        self.server.api_key = self.server.api_key.trim().to_string();
        self.qdrant.url = self.qdrant.url.trim().to_string();
        self.qdrant.api_key = self.qdrant.api_key.trim().to_string();
        self.embedding.local.model = self.embedding.local.model.trim().to_string();
        self.embedding.local.cache_dir = self.embedding.local.cache_dir.trim().to_string();
        self.embedding.local.query_prefix = self.embedding.local.query_prefix.trim().to_string();
        self.embedding.local.document_prefix =
            self.embedding.local.document_prefix.trim().to_string();
        self.embedding.api.base_url = self.embedding.api.base_url.trim().to_string();
        self.embedding.api.api_key = self.embedding.api.api_key.trim().to_string();
        self.embedding.api.model = self.embedding.api.model.trim().to_string();
    }
}

/// HTTP server settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub host: String,
    pub port: u16,
    pub api_key: String,
    pub max_body_mb: usize,
    pub request_timeout_secs: u64,
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            host: "0.0.0.0".to_string(),
            port: 8080,
            api_key: String::new(),
            max_body_mb: 32,
            request_timeout_secs: 300,
        }
    }
}

/// Qdrant connection settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct QdrantConfig {
    pub url: String,
    pub api_key: String,
    pub timeout_secs: u64,
    pub startup_retry_secs: u64,
}

impl Default for QdrantConfig {
    fn default() -> Self {
        Self {
            url: "http://localhost:6334".to_string(),
            api_key: String::new(),
            timeout_secs: 30,
            startup_retry_secs: 30,
        }
    }
}

/// Embedding provider selection and shared batching settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct EmbeddingConfig {
    pub provider: ProviderKind,
    pub batch_size: usize,
    pub max_concurrent_batches: usize,
    pub local: LocalEmbeddingConfig,
    pub api: ApiEmbeddingConfig,
}

impl Default for EmbeddingConfig {
    fn default() -> Self {
        Self {
            provider: ProviderKind::Local,
            batch_size: 32,
            max_concurrent_batches: 4,
            local: LocalEmbeddingConfig::default(),
            api: ApiEmbeddingConfig::default(),
        }
    }
}

/// Supported embedding provider kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ProviderKind {
    #[default]
    Local,
    Api,
}

/// In-process fastembed (ONNX) provider settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LocalEmbeddingConfig {
    pub model: String,
    pub cache_dir: String,
    pub show_download_progress: bool,
    pub max_length: usize,
    pub warmup: bool,
    pub query_prefix: String,
    pub document_prefix: String,
}

impl Default for LocalEmbeddingConfig {
    fn default() -> Self {
        Self {
            model: "bge-small-zh-v1.5".to_string(),
            cache_dir: "./data/models".to_string(),
            show_download_progress: true,
            max_length: 512,
            warmup: true,
            query_prefix: String::new(),
            document_prefix: String::new(),
        }
    }
}

/// OpenAI-compatible embedding API settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ApiEmbeddingConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub dimensions: usize,
    pub timeout_secs: u64,
    pub max_retries: usize,
}

impl Default for ApiEmbeddingConfig {
    fn default() -> Self {
        Self {
            base_url: "https://api.openai.com/v1".to_string(),
            api_key: String::new(),
            model: "text-embedding-3-small".to_string(),
            dimensions: 0,
            timeout_secs: 60,
            max_retries: 3,
        }
    }
}

/// Document ingestion settings.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct IngestConfig {
    pub chunk_size: usize,
    pub chunk_overlap: usize,
    pub min_chunk_size: usize,
    pub upsert_batch_size: usize,
}

impl Default for IngestConfig {
    fn default() -> Self {
        Self {
            chunk_size: 800,
            chunk_overlap: 120,
            min_chunk_size: 30,
            upsert_batch_size: 64,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::error::KbError;

    #[test]
    #[serial_test::serial]
    fn env_overrides_toml_and_defaults() {
        let path = std::env::temp_dir().join(format!("kb-config-{}.toml", uuid::Uuid::new_v4()));
        std::fs::write(
            &path,
            r#"
[server]
port = 9000

[embedding]
provider = "api"

[embedding.api]
base_url = "https://example.com/v1"
model = "embed-1"
"#,
        )
        .unwrap();
        std::env::set_var("KB_SERVER__PORT", "9100");
        std::env::set_var("KB_EMBEDDING__API__API_KEY", "sk-test");
        let cfg = AppConfig::load(Some(path.as_path())).unwrap();
        assert_eq!(cfg.server.port, 9100);
        assert_eq!(cfg.embedding.provider, ProviderKind::Api);
        assert_eq!(cfg.embedding.api.api_key, "sk-test");
        assert_eq!(cfg.ingest.chunk_size, 800);
        std::env::remove_var("KB_SERVER__PORT");
        std::env::remove_var("KB_EMBEDDING__API__API_KEY");
        let _ = std::fs::remove_file(path);
    }

    #[test]
    fn rejects_overlap_not_smaller_than_chunk_size() {
        let mut cfg = AppConfig::default();
        cfg.ingest.chunk_overlap = cfg.ingest.chunk_size;
        assert!(matches!(cfg.validate(), Err(KbError::Config(_))));
    }
}
