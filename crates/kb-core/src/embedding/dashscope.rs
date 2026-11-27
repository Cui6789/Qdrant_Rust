//! Native DashScope text-embedding provider.
//!
//! DashScope's native API is not OpenAI-compatible: it posts to
//! `{base_url}/services/embeddings/text-embedding/text-embedding` with
//! `{"model":..., "input":{"texts":[...]}, "parameters":{"text_type":...}}`.

use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::config::ApiEmbeddingConfig;
use crate::error::{KbError, Result};

/// DashScope's conservative batch limit across the supported text models.
const MAX_BATCH_SIZE: usize = 10;

/// DashScope native `/services/embeddings/text-embedding` provider.
pub struct DashScopeEmbedding {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    /// `dimension` parameter to send when explicitly configured.
    requested_dimension: Option<usize>,
    /// Resolved dimensionality of vectors produced by this provider.
    dimension: usize,
    max_retries: usize,
    batch_size: usize,
}

impl DashScopeEmbedding {
    /// Builds a provider and probes the model dimension when it is unknown.
    pub async fn new(config: &ApiEmbeddingConfig, batch_size: usize) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| {
                KbError::Embedding(format!("failed to build DashScope HTTP client: {error}"))
            })?;
        let endpoint = format!(
            "{}/services/embeddings/text-embedding/text-embedding",
            config.base_url.trim_end_matches('/')
        );
        let mut provider = Self {
            client,
            endpoint,
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            requested_dimension: (config.dimensions > 0).then_some(config.dimensions),
            dimension: config.dimensions,
            max_retries: config.max_retries,
            batch_size: batch_size.clamp(1, MAX_BATCH_SIZE),
        };
        if provider.dimension == 0 {
            let probe = ["dimension probe".to_string()];
            let vectors = provider.request_batch(&probe, "document").await?;
            let dimension = vectors
                .first()
                .map(Vec::len)
                .filter(|dimension| *dimension > 0)
                .ok_or_else(|| {
                    KbError::Embedding("DashScope returned an empty vector".to_string())
                })?;
            provider.dimension = dimension;
        }
        Ok(provider)
    }

    /// Sends one batch and returns vectors in input order.
    async fn request_batch(&self, texts: &[String], text_type: &str) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let body = DashScopeRequest {
            model: &self.model,
            input: DashScopeInput { texts },
            parameters: DashScopeParameters {
                text_type,
                dimension: self.requested_dimension,
            },
        };

        let mut retries = 0;
        loop {
            let mut request = self.client.post(&self.endpoint).json(&body);
            if !self.api_key.is_empty() {
                request = request.bearer_auth(&self.api_key);
            }

            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    let retryable = error.is_timeout() || error.is_connect();
                    if retryable && retries < self.max_retries {
                        retries += 1;
                        sleep_backoff(retries).await;
                        continue;
                    }
                    return Err(KbError::Embedding(format!(
                        "DashScope request failed: {}",
                        error.without_url()
                    )));
                }
            };

            let status = response.status();
            if !status.is_success() {
                if (status.as_u16() == 429 || status.is_server_error())
                    && retries < self.max_retries
                {
                    retries += 1;
                    sleep_backoff(retries).await;
                    continue;
                }
                return Err(KbError::Embedding(format!(
                    "DashScope API returned {status}"
                )));
            }

            let parsed: DashScopeResponse = response.json().await.map_err(|_| {
                KbError::Embedding("DashScope response was not valid JSON".to_string())
            })?;
            let mut embeddings = parsed
                .output
                .ok_or_else(|| {
                    KbError::Embedding("DashScope response did not contain output".to_string())
                })?
                .embeddings;
            embeddings.sort_by_key(|item| item.text_index);
            let vectors = embeddings
                .into_iter()
                .map(|item| item.embedding)
                .collect::<Vec<_>>();
            self.validate(texts.len(), &vectors)?;
            return Ok(vectors);
        }
    }

    /// Ensures the response matches the requested inputs and resolved dimension.
    fn validate(&self, expected: usize, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.len() != expected {
            return Err(KbError::Embedding(format!(
                "DashScope returned {} vectors for {expected} inputs",
                vectors.len()
            )));
        }
        if self.dimension > 0 {
            for (position, vector) in vectors.iter().enumerate() {
                if vector.len() != self.dimension {
                    return Err(KbError::Embedding(format!(
                        "DashScope vector {position} has {} dimensions, expected {}",
                        vector.len(),
                        self.dimension
                    )));
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl EmbeddingProvider for DashScopeEmbedding {
    fn model_id(&self) -> &str {
        &self.model
    }

    fn dimension(&self) -> usize {
        self.dimension
    }

    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        if texts.is_empty() {
            return Ok(Vec::new());
        }

        let mut vectors = Vec::with_capacity(texts.len());
        for batch in texts.chunks(self.batch_size) {
            vectors.extend(self.request_batch(batch, "document").await?);
        }
        Ok(vectors)
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let texts = [text.to_string()];
        let mut vectors = self.request_batch(&texts, "query").await?;
        vectors
            .pop()
            .ok_or_else(|| KbError::Embedding("DashScope returned no query vector".to_string()))
    }
}

#[derive(Serialize)]
struct DashScopeRequest<'a> {
    model: &'a str,
    input: DashScopeInput<'a>,
    parameters: DashScopeParameters<'a>,
}

#[derive(Serialize)]
struct DashScopeInput<'a> {
    texts: &'a [String],
}

#[derive(Serialize)]
struct DashScopeParameters<'a> {
    text_type: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    dimension: Option<usize>,
}

#[derive(Deserialize)]
struct DashScopeResponse {
    #[serde(default)]
    output: Option<DashScopeOutput>,
}

#[derive(Deserialize)]
struct DashScopeOutput {
    #[serde(default)]
    embeddings: Vec<DashScopeEmbeddingItem>,
}

#[derive(Deserialize)]
struct DashScopeEmbeddingItem {
    #[serde(default)]
    embedding: Vec<f32>,
    #[serde(default, alias = "index")]
    text_index: usize,
}

async fn sleep_backoff(attempt: usize) {
    let delay = 300u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::DashScopeEmbedding;
    use crate::config::{ApiEmbeddingConfig, EmbeddingProtocol};
    use crate::embedding::EmbeddingProvider;

    const API_KEY: &str = "dashscope-test-secret";
    const ENDPOINT: &str = "/api/v1/services/embeddings/text-embedding/text-embedding";

    fn config(server: &MockServer) -> ApiEmbeddingConfig {
        ApiEmbeddingConfig {
            base_url: format!("{}/api/v1/", server.uri()),
            api_key: API_KEY.to_string(),
            model: "test-embedding-model".to_string(),
            dimensions: 2,
            timeout_secs: 5,
            max_retries: 0,
            protocol: EmbeddingProtocol::Dashscope,
        }
    }

    #[tokio::test]
    async fn uses_native_protocol_for_documents_and_queries() {
        let server = MockServer::start().await;

        Mock::given(method("POST"))
            .and(path(ENDPOINT))
            .and(header(
                "Authorization",
                format!("Bearer {API_KEY}").as_str(),
            ))
            .and(body_json(json!({
                "model": "test-embedding-model",
                "input": {"texts": ["first", "second"]},
                "parameters": {
                    "text_type": "document",
                    "dimension": 2
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": {
                    "embeddings": [
                        {"text_index": 1, "embedding": [3.0, 4.0]},
                        {"text_index": 0, "embedding": [1.0, 2.0]}
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("POST"))
            .and(path(ENDPOINT))
            .and(header(
                "Authorization",
                format!("Bearer {API_KEY}").as_str(),
            ))
            .and(body_json(json!({
                "model": "test-embedding-model",
                "input": {"texts": ["find it"]},
                "parameters": {
                    "text_type": "query",
                    "dimension": 2
                }
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "output": {
                    "embeddings": [
                        {"index": 0, "embedding": [5.0, 6.0]}
                    ]
                }
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = DashScopeEmbedding::new(&config(&server), 10).await.unwrap();
        let documents = provider
            .embed_documents(&["first".to_string(), "second".to_string()])
            .await
            .unwrap();
        let query = provider.embed_query("find it").await.unwrap();

        assert_eq!(documents, vec![vec![1.0, 2.0], vec![3.0, 4.0]]);
        assert_eq!(query, vec![5.0, 6.0]);
        assert_eq!(provider.dimension(), 2);
    }

    #[tokio::test]
    async fn errors_do_not_expose_credentials_endpoint_or_response_body() {
        let server = MockServer::start().await;
        let leaked_endpoint = format!("{}{ENDPOINT}", server.uri());
        let leaked_body =
            format!("password=leaked-password endpoint={leaked_endpoint} key={API_KEY}");

        Mock::given(method("POST"))
            .and(path(ENDPOINT))
            .respond_with(ResponseTemplate::new(401).set_body_string(leaked_body))
            .expect(1)
            .mount(&server)
            .await;

        let provider = DashScopeEmbedding::new(&config(&server), 10).await.unwrap();
        let error = provider
            .embed_query("find it")
            .await
            .unwrap_err()
            .to_string();

        assert!(error.contains("401"), "{error}");
        assert!(!error.contains(API_KEY));
        assert!(!error.contains(&server.uri()));
        assert!(!error.contains(ENDPOINT));
        assert!(!error.contains("leaked-password"));
    }
}
