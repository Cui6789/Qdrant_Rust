use std::collections::BTreeMap;

use kb_core::config::{AppConfig, EmbeddingProtocol, ProviderKind};
use kb_core::embedding::EmbeddingProvider;
use kb_core::error::{KbError, Result};
use kb_core::ingest::upload::ParsingProvider;
use serde::{Deserialize, Serialize};

/// Missing values inherit the saved or startup value. An empty key clears it.
#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default, deny_unknown_fields)]
pub struct SettingsPatch {
    embedding_provider: Option<ProviderKind>,
    embedding_local_model: Option<String>,
    embedding_local_dimensions: Option<usize>,
    embedding_api_protocol: Option<EmbeddingProtocol>,
    embedding_api_base_url: Option<String>,
    embedding_api_model: Option<String>,
    embedding_api_dimensions: Option<usize>,
    embedding_api_key: Option<String>,
    embedding_mock_dimensions: Option<usize>,
    rerank_provider: Option<String>,
    rerank_candidate_limit: Option<u64>,
    rerank_local_model: Option<String>,
    rerank_api_endpoint: Option<String>,
    rerank_api_model: Option<String>,
    rerank_api_key: Option<String>,
    parsing_provider: Option<ParsingProvider>,
    mineru_local_url: Option<String>,
    mineru_cloud_url: Option<String>,
    mineru_model_version: Option<String>,
    mineru_api_key: Option<String>,
}

impl SettingsPatch {
    pub(super) fn clear_embedding(&mut self) {
        self.embedding_provider = None;
        self.embedding_local_model = None;
        self.embedding_local_dimensions = None;
        self.embedding_api_protocol = None;
        self.embedding_api_base_url = None;
        self.embedding_api_model = None;
        self.embedding_api_dimensions = None;
        self.embedding_api_key = None;
        self.embedding_mock_dimensions = None;
    }

    /// Apply a new patch over the persisted overlay, then retain that overlay.
    pub(super) fn apply(mut self, saved: Self, config: &mut AppConfig) -> Result<Self> {
        for key in [
            &mut self.embedding_api_key,
            &mut self.rerank_api_key,
            &mut self.mineru_api_key,
        ]
        .into_iter()
        .flatten()
        {
            *key = key.trim().to_owned();
            if key.len() > 4096 || key.bytes().any(|byte| !(0x21..=0x7e).contains(&byte)) {
                return Err(KbError::Validation(
                    "API key contains invalid characters".into(),
                ));
            }
        }

        for value in [
            &mut self.embedding_local_model,
            &mut self.embedding_api_base_url,
            &mut self.embedding_api_model,
            &mut self.rerank_provider,
            &mut self.rerank_local_model,
            &mut self.rerank_api_endpoint,
            &mut self.rerank_api_model,
            &mut self.mineru_local_url,
            &mut self.mineru_cloud_url,
            &mut self.mineru_model_version,
        ]
        .into_iter()
        .flatten()
        {
            *value = value.trim().to_owned();
        }

        fn apply<T: Clone>(incoming: Option<T>, saved: Option<T>, target: &mut T) -> Option<T> {
            let value = incoming.or(saved);
            if let Some(value) = &value {
                *target = value.clone();
            }
            value
        }

        Ok(Self {
            embedding_provider: apply(
                self.embedding_provider,
                saved.embedding_provider,
                &mut config.embedding.provider,
            ),
            embedding_local_model: apply(
                self.embedding_local_model,
                saved.embedding_local_model,
                &mut config.embedding.local.model,
            ),
            embedding_local_dimensions: apply(
                self.embedding_local_dimensions,
                saved.embedding_local_dimensions,
                &mut config.embedding.local.dimensions,
            ),
            embedding_api_protocol: apply(
                self.embedding_api_protocol,
                saved.embedding_api_protocol,
                &mut config.embedding.api.protocol,
            ),
            embedding_api_base_url: apply(
                self.embedding_api_base_url,
                saved.embedding_api_base_url,
                &mut config.embedding.api.base_url,
            ),
            embedding_api_model: apply(
                self.embedding_api_model,
                saved.embedding_api_model,
                &mut config.embedding.api.model,
            ),
            embedding_api_dimensions: apply(
                self.embedding_api_dimensions,
                saved.embedding_api_dimensions,
                &mut config.embedding.api.dimensions,
            ),
            embedding_api_key: apply(
                self.embedding_api_key,
                saved.embedding_api_key,
                &mut config.embedding.api.api_key,
            ),
            embedding_mock_dimensions: apply(
                self.embedding_mock_dimensions,
                saved.embedding_mock_dimensions,
                &mut config.embedding.mock_dimensions,
            ),
            rerank_provider: apply(
                self.rerank_provider,
                saved.rerank_provider,
                &mut config.rerank.provider,
            ),
            rerank_candidate_limit: apply(
                self.rerank_candidate_limit,
                saved.rerank_candidate_limit,
                &mut config.rerank.candidate_limit,
            ),
            rerank_local_model: apply(
                self.rerank_local_model,
                saved.rerank_local_model,
                &mut config.rerank.local.model,
            ),
            rerank_api_endpoint: apply(
                self.rerank_api_endpoint,
                saved.rerank_api_endpoint,
                &mut config.rerank.api.endpoint,
            ),
            rerank_api_model: apply(
                self.rerank_api_model,
                saved.rerank_api_model,
                &mut config.rerank.api.model,
            ),
            rerank_api_key: apply(
                self.rerank_api_key,
                saved.rerank_api_key,
                &mut config.rerank.api.api_key,
            ),
            parsing_provider: apply(
                self.parsing_provider,
                saved.parsing_provider,
                &mut config.parsing.provider,
            ),
            mineru_local_url: apply(
                self.mineru_local_url,
                saved.mineru_local_url,
                &mut config.parsing.mineru.local_url,
            ),
            mineru_cloud_url: apply(
                self.mineru_cloud_url,
                saved.mineru_cloud_url,
                &mut config.parsing.mineru.cloud_url,
            ),
            mineru_model_version: apply(
                self.mineru_model_version,
                saved.mineru_model_version,
                &mut config.parsing.mineru.model_version,
            ),
            mineru_api_key: apply(
                self.mineru_api_key,
                saved.mineru_api_key,
                &mut config.parsing.mineru.api_key,
            ),
        })
    }
}

#[derive(Serialize)]
pub struct SettingsView {
    embedding_provider: ProviderKind,
    embedding_model: String,
    embedding_dimension: usize,
    embedding_local_model: String,
    embedding_local_dimensions: usize,
    embedding_local_model_dimensions: BTreeMap<&'static str, usize>,
    embedding_api_protocol: EmbeddingProtocol,
    embedding_api_base_url: String,
    embedding_api_model: String,
    embedding_api_dimensions: usize,
    embedding_api_key_set: bool,
    embedding_mock_dimensions: usize,
    rerank_provider: String,
    rerank_candidate_limit: u64,
    rerank_local_model: String,
    rerank_api_endpoint: String,
    rerank_api_model: String,
    rerank_api_key_set: bool,
    parsing_provider: ParsingProvider,
    mineru_local_url: String,
    mineru_cloud_url: String,
    mineru_model_version: String,
    mineru_api_key_set: bool,
}

impl SettingsView {
    pub(super) fn new(config: &AppConfig, embedder: &dyn EmbeddingProvider) -> Self {
        let embedding = &config.embedding;
        let rerank = &config.rerank;
        let parsing = &config.parsing;
        #[cfg(feature = "local-embedding")]
        let embedding_local_model_dimensions =
            kb_core::embedding::local::supported_model_dimensions().collect();
        #[cfg(not(feature = "local-embedding"))]
        let embedding_local_model_dimensions = BTreeMap::new();
        Self {
            embedding_provider: embedding.provider,
            embedding_model: embedder.model_id().to_owned(),
            embedding_dimension: embedder.dimension(),
            embedding_local_model: embedding.local.model.clone(),
            embedding_local_dimensions: embedding.local.dimensions,
            embedding_local_model_dimensions,
            embedding_api_protocol: embedding.api.protocol,
            embedding_api_base_url: embedding.api.base_url.clone(),
            embedding_api_model: embedding.api.model.clone(),
            embedding_api_dimensions: embedding.api.dimensions,
            embedding_api_key_set: !embedding.api.api_key.trim().is_empty(),
            embedding_mock_dimensions: embedding.mock_dimensions,
            rerank_provider: rerank.provider.clone(),
            rerank_candidate_limit: rerank.candidate_limit,
            rerank_local_model: rerank.local.model.clone(),
            rerank_api_endpoint: rerank.api.endpoint.clone(),
            rerank_api_model: rerank.api.model.clone(),
            rerank_api_key_set: !rerank.api.api_key.trim().is_empty(),
            parsing_provider: parsing.provider,
            mineru_local_url: parsing.mineru.local_url.clone(),
            mineru_cloud_url: parsing.mineru.cloud_url.clone(),
            mineru_model_version: parsing.mineru.model_version.clone(),
            mineru_api_key_set: !parsing.mineru.api_key.trim().is_empty(),
        }
    }
}
