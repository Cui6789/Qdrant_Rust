//! In-process multilingual reranking backed by fastembed.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use fastembed::{RerankInitOptions, RerankerModel, TextRerank};

use super::{
    local_model_code, validate_ranked_documents, LocalRerankConfig, RankedDocument, RerankProvider,
};
use crate::error::{KbError, Result};

/// Local fastembed reranker. ONNX inference runs on a blocking thread.
pub(super) struct LocalRerank {
    model: Arc<Mutex<TextRerank>>,
}

impl LocalRerank {
    pub(super) fn new(config: &LocalRerankConfig) -> Result<Self> {
        let model = resolve_model(&config.model)?;
        let options = RerankInitOptions::new(model)
            .with_cache_dir(PathBuf::from(config.cache_dir.trim()))
            .with_show_download_progress(false);
        let model = TextRerank::try_new(options).map_err(|error| {
            KbError::Rerank(format!("failed to load local rerank model: {error}"))
        })?;
        Ok(Self {
            model: Arc::new(Mutex::new(model)),
        })
    }
}

#[async_trait::async_trait]
impl RerankProvider for LocalRerank {
    async fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RankedDocument>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }

        let expected_count = documents.len();
        let query = query.to_string();
        let documents: Vec<String> = documents
            .iter()
            .map(|document| (*document).to_string())
            .collect();
        let model = Arc::clone(&self.model);

        tokio::task::spawn_blocking(move || -> Result<Vec<RankedDocument>> {
            let mut model = model
                .lock()
                .map_err(|_| KbError::Rerank("local rerank model lock is poisoned".to_string()))?;
            let document_refs: Vec<&str> = documents.iter().map(String::as_str).collect();
            let results = model
                .rerank(query.as_str(), document_refs.as_slice(), false, None)
                .map_err(|error| {
                    KbError::Rerank(format!("local rerank inference failed: {error}"))
                })?;
            let ranked: Vec<RankedDocument> = results
                .into_iter()
                .map(|result| RankedDocument {
                    index: result.index,
                    score: result.score,
                })
                .collect();
            validate_ranked_documents(expected_count, ranked)
        })
        .await
        .map_err(|error| KbError::Rerank(format!("local rerank task failed: {error}")))?
    }
}

fn resolve_model(alias: &str) -> Result<RerankerModel> {
    let model_code = local_model_code(alias).ok_or_else(|| {
        KbError::Config(
            "rerank.local.model is not supported by the bundled fastembed rerankers".to_string(),
        )
    })?;
    model_code.parse::<RerankerModel>().map_err(|_| {
        KbError::Config(
            "rerank.local.model is not supported by the bundled fastembed rerankers".to_string(),
        )
    })
}

#[cfg(test)]
mod tests {
    use fastembed::RerankerModel;

    use super::resolve_model;
    use crate::rerank::{LocalRerankConfig, RerankProvider};

    #[test]
    fn resolves_fastembed_reranker_aliases() {
        assert_eq!(
            resolve_model("bge-reranker-base").unwrap(),
            RerankerModel::BGERerankerBase
        );
        assert_eq!(
            resolve_model("bge-reranker-v2-m3").unwrap(),
            RerankerModel::BGERerankerV2M3
        );
        assert_eq!(
            resolve_model("jina-reranker-v1-turbo-en").unwrap(),
            RerankerModel::JINARerankerV1TurboEn
        );
        assert_eq!(
            resolve_model("jina-reranker-v2-base-multilingual").unwrap(),
            RerankerModel::JINARerankerV2BaseMultilingual
        );
    }

    #[tokio::test]
    #[ignore = "downloads the multilingual bge-reranker-v2-m3 model from Hugging Face"]
    async fn reranks_with_local_multilingual_bge() {
        let config = LocalRerankConfig::default();
        let provider = super::LocalRerank::new(&config).unwrap();
        let ranked = provider
            .rerank(
                "How do I request annual leave?",
                &[
                    "Use the HR portal to request annual leave.",
                    "The office opens at nine.",
                ],
            )
            .await
            .unwrap();

        assert_eq!(ranked.len(), 2);
        assert_eq!(
            ranked.iter().map(|item| item.index).collect::<Vec<_>>(),
            vec![0, 1]
        );
        assert!(ranked.iter().all(|item| item.score.is_finite()));
    }
}
