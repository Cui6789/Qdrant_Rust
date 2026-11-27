use std::time::Duration;

use futures::stream::{self, StreamExt};
use serde::{Deserialize, Serialize};

use super::EmbeddingProvider;
use crate::config::ApiEmbeddingConfig;
use crate::error::{KbError, Result};

/// Base delay for exponential backoff between retries.
const BASE_RETRY_DELAY_MS: u64 = 250;
/// Upper bound for the exponential backoff delay.
const MAX_RETRY_DELAY: Duration = Duration::from_secs(5);
/// Embedding provider backed by an OpenAI-compatible `/embeddings` endpoint.
pub struct ApiEmbedding {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    /// `dimensions` parameter to send with requests, when explicitly configured.
    requested_dimensions: Option<usize>,
    /// Resolved dimensionality of the vectors this provider produces.
    dimension: usize,
    max_retries: usize,
    batch_size: usize,
    max_concurrent_batches: usize,
}

impl ApiEmbedding {
    /// Builds a provider from `config`, probing the API for its dimension when
    /// `config.dimensions` is zero.
    pub async fn new(
        config: &ApiEmbeddingConfig,
        batch_size: usize,
        max_concurrent_batches: usize,
    ) -> Result<Self> {
        let mut provider = Self::with_known_dimension(
            config,
            batch_size,
            max_concurrent_batches,
            config.dimensions,
        )?;
        if provider.dimension == 0 {
            let probe = ["dimension probe".to_string()];
            let vectors = provider.request_batch(&probe).await?;
            let dimension = vectors.first().map(Vec::len).ok_or_else(|| {
                KbError::Embedding("embedding API returned no vectors".to_string())
            })?;
            if dimension == 0 {
                return Err(KbError::Embedding(
                    "embedding API returned an empty vector".to_string(),
                ));
            }
            provider.dimension = dimension;
        }
        Ok(provider)
    }

    /// Uses a dimension already resolved for this model. The configured
    /// dimensions argument still controls the wire format.
    pub fn with_known_dimension(
        config: &ApiEmbeddingConfig,
        batch_size: usize,
        max_concurrent_batches: usize,
        dimension: usize,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .redirect(reqwest::redirect::Policy::none())
            .build()
            .map_err(|error| KbError::Embedding(format!("failed to build HTTP client: {error}")))?;

        Ok(Self {
            client,
            endpoint: format!("{}/embeddings", config.base_url.trim_end_matches('/')),
            api_key: config.api_key.clone(),
            model: config.model.clone(),
            requested_dimensions: (config.dimensions > 0).then_some(config.dimensions),
            dimension,
            max_retries: config.max_retries,
            batch_size: batch_size.max(1),
            max_concurrent_batches: max_concurrent_batches.max(1),
        })
    }

    /// Embeds a single batch, retrying transient failures.
    async fn request_batch(&self, texts: &[String]) -> Result<Vec<Vec<f32>>> {
        let body = EmbeddingRequest {
            model: &self.model,
            input: texts,
            dimensions: self.requested_dimensions,
        };

        let mut retries = 0;
        loop {
            match self.send_once(&body).await {
                Ok(vectors) => return self.validate(texts.len(), vectors),
                Err(failure) => {
                    if !failure.retryable || retries >= self.max_retries {
                        return Err(KbError::Embedding(failure.message));
                    }
                    let delay = failure
                        .retry_after
                        .unwrap_or_else(|| backoff_delay(retries));
                    tracing::warn!(
                        attempt = retries + 1,
                        max_retries = self.max_retries,
                        status = failure.status.map(|status| status.as_u16()),
                        delay = ?delay,
                        error = %failure.message,
                        "retrying embedding request"
                    );
                    tokio::time::sleep(delay).await;
                    retries += 1;
                }
            }
        }
    }

    /// Sends one embedding request, classifying failures as retryable or fatal.
    async fn send_once(
        &self,
        body: &EmbeddingRequest<'_>,
    ) -> std::result::Result<Vec<Vec<f32>>, AttemptFailure> {
        let mut request = self.client.post(&self.endpoint).json(body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }

        let response = match request.send().await {
            Ok(response) => response,
            Err(error) => {
                let retryable = error.is_timeout() || error.is_connect();
                return Err(AttemptFailure {
                    message: format!("embedding request failed: {}", error.without_url()),
                    retryable,
                    retry_after: None,
                    status: None,
                });
            }
        };

        let status = response.status();
        let retry_after = retry_after_delay(&response);
        if !status.is_success() {
            return Err(AttemptFailure {
                message: format!("embedding API returned {status}"),
                retryable: status.as_u16() == 429 || status.is_server_error(),
                retry_after,
                status: Some(status),
            });
        }

        let response: EmbeddingResponse = response.json().await.map_err(|_| AttemptFailure {
            message: "invalid embedding response".into(),
            retryable: false,
            retry_after: None,
            status: Some(status),
        })?;

        let mut data = response.data;
        data.sort_by_key(|item| item.index);
        Ok(data.into_iter().map(|item| item.embedding).collect())
    }

    /// Ensures the response matches the requested inputs and resolved dimension.
    fn validate(&self, expected_count: usize, vectors: Vec<Vec<f32>>) -> Result<Vec<Vec<f32>>> {
        if vectors.len() != expected_count {
            return Err(KbError::Embedding(format!(
                "embedding API returned {} vectors for {expected_count} inputs",
                vectors.len()
            )));
        }
        if self.dimension > 0 {
            for (position, vector) in vectors.iter().enumerate() {
                if vector.len() != self.dimension {
                    return Err(KbError::Embedding(format!(
                        "embedding {position} has {} dimensions, expected {}",
                        vector.len(),
                        self.dimension
                    )));
                }
            }
        }
        Ok(vectors)
    }
}

#[async_trait::async_trait]
impl EmbeddingProvider for ApiEmbedding {
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

        let batch_count = texts.len().div_ceil(self.batch_size);
        let results = stream::iter(0..batch_count)
            .map(|batch_index: usize| async move {
                let start = batch_index * self.batch_size;
                let end = (start + self.batch_size).min(texts.len());
                let vectors = self.request_batch(&texts[start..end]).await?;
                Ok::<(usize, Vec<Vec<f32>>), KbError>((batch_index, vectors))
            })
            .buffer_unordered(self.max_concurrent_batches)
            .collect::<Vec<_>>()
            .await;

        let mut batches: Vec<(usize, Vec<Vec<f32>>)> = Vec::with_capacity(results.len());
        for result in results {
            batches.push(result?);
        }
        batches.sort_by_key(|(batch_index, _)| *batch_index);
        Ok(batches
            .into_iter()
            .flat_map(|(_, vectors)| vectors)
            .collect())
    }

    async fn embed_query(&self, text: &str) -> Result<Vec<f32>> {
        let vectors = self.embed_documents(&[text.to_string()]).await?;
        vectors
            .into_iter()
            .next()
            .ok_or_else(|| KbError::Embedding("embedding API returned no vector".to_string()))
    }
}

/// Body of an OpenAI-compatible embeddings request.
#[derive(Serialize)]
struct EmbeddingRequest<'a> {
    model: &'a str,
    input: &'a [String],
    #[serde(skip_serializing_if = "Option::is_none")]
    dimensions: Option<usize>,
}

/// OpenAI-compatible embeddings response.
#[derive(Deserialize)]
struct EmbeddingResponse {
    data: Vec<EmbeddingData>,
}

/// One embedding entry in a response.
#[derive(Deserialize)]
struct EmbeddingData {
    embedding: Vec<f32>,
    index: usize,
}

/// A failed attempt; `retryable` marks transient failures.
struct AttemptFailure {
    message: String,
    retryable: bool,
    retry_after: Option<Duration>,
    status: Option<reqwest::StatusCode>,
}

/// Exponential backoff: `min(250ms * 2^attempt, 5s)`.
fn backoff_delay(attempt: usize) -> Duration {
    let exponent = u32::try_from(attempt).unwrap_or(u32::MAX).min(16);
    Duration::from_millis(BASE_RETRY_DELAY_MS.saturating_mul(1u64 << exponent)).min(MAX_RETRY_DELAY)
}

/// Reads a numeric `Retry-After` header, in seconds.
fn retry_after_delay(response: &reqwest::Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?;
    Duration::try_from_secs_f64(value.trim().parse::<f64>().ok()?).ok()
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::ApiEmbedding;
    use crate::config::ApiEmbeddingConfig;
    use crate::embedding::EmbeddingProvider;

    fn config(server: &MockServer) -> ApiEmbeddingConfig {
        ApiEmbeddingConfig {
            protocol: Default::default(),
            base_url: format!("{}/v1/", server.uri()),
            api_key: "sk-test".to_string(),
            model: "test-model".to_string(),
            dimensions: 2,
            timeout_secs: 5,
            max_retries: 0,
        }
    }

    #[tokio::test]
    async fn embeds_batches_in_order_with_auth() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .and(header("Authorization", "Bearer sk-test"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [
                    {"index": 1, "embedding": [0.0, 1.0]},
                    {"index": 0, "embedding": [3.0, 4.0]}
                ]
            })))
            .mount(&server)
            .await;

        let provider = ApiEmbedding::new(&config(&server), 8, 2).await.unwrap();
        let vectors = provider
            .embed_documents(&["a".to_string(), "b".to_string()])
            .await
            .unwrap();

        assert_eq!(vectors, vec![vec![3.0, 4.0], vec![0.0, 1.0]]);
        assert_eq!(provider.dimension(), 2);
    }

    #[tokio::test]
    async fn splits_into_batches() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(|request: &wiremock::Request| {
                let body: serde_json::Value =
                    serde_json::from_slice(&request.body).expect("request body is JSON");
                let inputs = body["input"].as_array().expect("input is an array");
                let data: Vec<serde_json::Value> = inputs
                    .iter()
                    .enumerate()
                    .map(|(index, _)| json!({"index": index, "embedding": [1.0, 2.0]}))
                    .collect();
                ResponseTemplate::new(200).set_body_json(json!({"data": data}))
            })
            .expect(3)
            .mount(&server)
            .await;

        let provider = ApiEmbedding::new(&config(&server), 2, 2).await.unwrap();
        let inputs: Vec<String> = (0..5).map(|index| format!("text-{index}")).collect();
        let vectors = provider.embed_documents(&inputs).await.unwrap();

        assert_eq!(vectors.len(), 5);
    }

    #[tokio::test]
    async fn retries_on_429_then_succeeds() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .up_to_n_times(1)
            .mount(&server)
            .await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"index": 0, "embedding": [0.5, 0.5]}]
            })))
            .mount(&server)
            .await;

        let mut config = config(&server);
        config.max_retries = 2;
        let provider = ApiEmbedding::new(&config, 8, 2).await.unwrap();
        let vectors = provider.embed_documents(&["a".to_string()]).await.unwrap();

        assert_eq!(vectors, vec![vec![0.5, 0.5]]);
    }

    #[tokio::test]
    async fn non_2xx_is_embedding_error() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(500).set_body_string("boom"))
            .mount(&server)
            .await;

        let provider = ApiEmbedding::new(&config(&server), 8, 2).await.unwrap();
        let error = provider
            .embed_documents(&["a".to_string()])
            .await
            .unwrap_err();

        assert!(matches!(error, crate::error::KbError::Embedding(_)));
        assert!(error.to_string().contains("500"));
    }

    #[tokio::test]
    async fn probes_dimension_when_config_is_zero() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = config(&server);
        config.dimensions = 0;
        let provider = ApiEmbedding::new(&config, 8, 2).await.unwrap();

        assert_eq!(provider.dimension(), 3);
    }

    #[tokio::test]
    async fn rejects_wrong_dimension() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/embeddings"))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "data": [{"index": 0, "embedding": [0.1, 0.2, 0.3]}]
            })))
            .mount(&server)
            .await;

        let provider = ApiEmbedding::new(&config(&server), 8, 2).await.unwrap();
        let error = provider
            .embed_documents(&["a".to_string()])
            .await
            .unwrap_err();

        assert!(matches!(error, crate::error::KbError::Embedding(_)));
    }
}
