//! Runtime provider settings shared by the management UI, REST API and workbench.

mod probes;
mod settings;

use std::io::{ErrorKind, Write};
use std::path::Path;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant};

use kb_core::config::{AppConfig, EmbeddingConfig, ProviderKind};
use kb_core::embedding::EmbeddingProvider;
use kb_core::error::{KbError, Result};
use kb_core::ingest::upload::UploadParser;
use kb_core::rerank::{build_reranker, RerankProvider};
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;
use serde::Serialize;
use subtle::ConstantTimeEq;

use crate::build_embedder;

pub(crate) use probes::{ParsingProbe, RerankProbe};
pub use settings::{SettingsPatch, SettingsView};

#[derive(Clone)]
struct ActiveSettings {
    config: AppConfig,
    overlay: SettingsPatch,
    service: Arc<KnowledgeBaseService>,
    reranker: Option<Arc<dyn RerankProvider>>,
}

impl ActiveSettings {
    fn view(&self) -> SettingsView {
        SettingsView::new(&self.config, self.service.embedder().as_ref())
    }
}

#[derive(Serialize)]
pub(crate) struct EmbeddingProbe {
    model: String,
    dimension: usize,
    latency_ms: u64,
    sample: Vec<f32>,
    norm: f64,
}

/// Owns the effective providers and serializes updates for this server process.
pub struct Management {
    active: RwLock<ActiveSettings>,
    updates: tokio::sync::Mutex<()>,
    access_key: String,
    upload_timeout: Duration,
    startup_embedding: EmbeddingConfig,
}

impl Management {
    pub async fn new(mut config: AppConfig, store: Arc<QdrantStore>) -> Result<Self> {
        let startup_embedding = config.embedding.clone();
        let overlay = load(&config.admin.settings_path)?;
        let overlay = overlay.apply(SettingsPatch::default(), &mut config)?;
        config.validate()?;

        // The chunk size counts characters while a local model's window counts tokens.
        if config.embedding.provider == ProviderKind::Local
            && config.ingest.chunk_size > config.embedding.local.max_length
        {
            tracing::warn!(
                chunk_size = config.ingest.chunk_size,
                max_length = config.embedding.local.max_length,
                "local model truncates chunks that exceed its token window; consider a smaller ingest.chunk_size"
            );
        }

        let upload = Arc::new(UploadParser::new(&config.parsing)?);
        let reranker = build_reranker(&config.rerank).await?;
        let embedder = build_embedder(&config.embedding).await?;
        let service = KnowledgeBaseService::new(store, embedder.clone(), config.ingest.clone())
            .with_runtime(
                embedder,
                reranker.clone(),
                config.rerank.candidate_limit,
                upload,
            );
        let access_key = config.admin.access_key.clone();
        let upload_timeout = Duration::from_secs(
            config
                .server
                .request_timeout_secs
                .saturating_add(config.parsing.mineru.timeout_secs),
        );

        Ok(Self {
            active: RwLock::new(ActiveSettings {
                config,
                overlay,
                service: Arc::new(service),
                reranker,
            }),
            updates: tokio::sync::Mutex::new(()),
            access_key,
            upload_timeout,
            startup_embedding,
        })
    }

    pub fn service(&self) -> Arc<KnowledgeBaseService> {
        self.active
            .read()
            .expect("settings lock poisoned")
            .service
            .clone()
    }

    /// The provider kind and service are read from the same runtime snapshot.
    pub fn snapshot(&self) -> (Arc<KnowledgeBaseService>, ProviderKind) {
        let active = self.active.read().expect("settings lock poisoned");
        (active.service.clone(), active.config.embedding.provider)
    }

    pub(crate) fn upload_timeout(&self) -> Duration {
        self.upload_timeout
    }

    pub(crate) fn enabled(&self) -> bool {
        !self.access_key.is_empty()
    }

    pub(crate) fn accepts(&self, token: &str) -> bool {
        self.enabled() && bool::from(self.access_key.as_bytes().ct_eq(token.as_bytes()))
    }

    pub(crate) fn view(&self) -> SettingsView {
        self.active.read().expect("settings lock poisoned").view()
    }

    pub(crate) async fn apply(&self, patch: SettingsPatch) -> Result<SettingsView> {
        let _update = self.updates.lock().await;
        let current = self.active.read().expect("settings lock poisoned").clone();
        let mut config = current.config.clone();
        let overlay = patch.apply(current.overlay.clone(), &mut config)?;
        self.replace(current, config, overlay).await
    }

    pub(crate) async fn reset_embedding(&self) -> Result<SettingsView> {
        let _update = self.updates.lock().await;
        let current = self.active.read().expect("settings lock poisoned").clone();
        let mut config = current.config.clone();
        config.embedding = self.startup_embedding.clone();
        let mut overlay = current.overlay.clone();
        overlay.clear_embedding();
        self.replace(current, config, overlay).await
    }

    pub(crate) async fn probe_embedding(&self) -> Result<EmbeddingProbe> {
        probe(self.service().embedder().as_ref()).await
    }

    pub(crate) async fn probe_rerank(&self) -> Result<RerankProbe> {
        let (provider, model, reranker) = {
            let active = self.active.read().expect("settings lock poisoned");
            let rerank = &active.config.rerank;
            let provider = rerank.provider.trim().to_ascii_lowercase();
            let model = if provider == "local" {
                rerank.local.model.clone()
            } else {
                rerank.api.model.clone()
            };
            (provider, model, active.reranker.clone())
        };
        let reranker =
            reranker.ok_or_else(|| KbError::Validation("reranking is disabled".into()))?;
        probes::rerank(reranker.as_ref(), provider, model).await
    }

    pub(crate) async fn probe_parsing(&self) -> Result<ParsingProbe> {
        let (provider, cloud_model, service) = {
            let active = self.active.read().expect("settings lock poisoned");
            let parsing = &active.config.parsing;
            (
                parsing.provider,
                parsing.mineru.model_version.trim().to_owned(),
                active.service.clone(),
            )
        };
        probes::parsing(&service, provider, &cloud_model).await
    }

    async fn replace(
        &self,
        current: ActiveSettings,
        config: AppConfig,
        overlay: SettingsPatch,
    ) -> Result<SettingsView> {
        config.validate().map_err(settings_validation)?;

        let embedder = if embedding_changed(&current.config.embedding, &config.embedding) {
            let embedding = &config.embedding;
            let provider = build_embedder(embedding)
                .await
                .map_err(settings_validation)?;
            if embedding.provider == ProviderKind::Api && embedding.api.dimensions != 0 {
                probe(provider.as_ref()).await?;
            }
            provider
        } else {
            current.service.embedder()
        };

        let mut previous_rerank = current.config.rerank;
        previous_rerank.candidate_limit = config.rerank.candidate_limit;
        let reranker = if config.rerank == previous_rerank {
            current.reranker
        } else {
            build_reranker(&config.rerank)
                .await
                .map_err(settings_validation)?
        };
        let upload = Arc::new(UploadParser::new(&config.parsing).map_err(settings_validation)?);
        let service = Arc::new(current.service.with_runtime(
            embedder,
            reranker.clone(),
            config.rerank.candidate_limit,
            upload,
        ));

        // Once the file is replaced, publish the matching snapshot before yielding.
        persist(&config.admin.settings_path, &overlay)?;
        let updated = ActiveSettings {
            config,
            overlay,
            service,
            reranker,
        };
        let view = updated.view();
        *self.active.write().expect("settings lock poisoned") = updated;
        Ok(view)
    }
}

fn embedding_changed(previous: &EmbeddingConfig, next: &EmbeddingConfig) -> bool {
    previous.provider != next.provider
        || match next.provider {
            ProviderKind::Local => previous.local != next.local,
            ProviderKind::Api => previous.api != next.api,
            ProviderKind::Mock => previous.mock_dimensions != next.mock_dimensions,
        }
}

async fn probe(embedder: &dyn EmbeddingProvider) -> Result<EmbeddingProbe> {
    let started = Instant::now();
    let vector = embedder.embed_query("Qdrant Rust embedding test").await?;
    if vector.len() != embedder.dimension() || vector.iter().any(|value| !value.is_finite()) {
        return Err(KbError::Embedding(
            "model returned an invalid vector".into(),
        ));
    }
    Ok(EmbeddingProbe {
        model: embedder.model_id().to_owned(),
        dimension: vector.len(),
        latency_ms: started.elapsed().as_millis() as u64,
        norm: vector
            .iter()
            .map(|&value| f64::from(value).powi(2))
            .sum::<f64>()
            .sqrt(),
        sample: vector.into_iter().take(16).collect(),
    })
}

fn settings_validation(error: KbError) -> KbError {
    match error {
        KbError::Config(message) => KbError::Validation(message),
        error => error,
    }
}

fn load(path: &Path) -> Result<SettingsPatch> {
    match std::fs::read(path) {
        Ok(bytes) => serde_json::from_slice(&bytes)
            .map_err(|_| KbError::Config("saved admin settings are invalid".into())),
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(SettingsPatch::default()),
        Err(error) => Err(error.into()),
    }
}

fn persist(path: &Path, overlay: &SettingsPatch) -> Result<()> {
    let dir = path
        .parent()
        .filter(|path| !path.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir)?;
    let mut file = tempfile::NamedTempFile::new_in(dir)?;
    serde_json::to_writer(&mut file, overlay)
        .map_err(|_| KbError::Internal("failed to write admin settings".into()))?;
    file.write_all(b"\n")?;
    file.as_file().sync_all()?;
    file.persist(path).map_err(|error| error.error)?;
    Ok(())
}
