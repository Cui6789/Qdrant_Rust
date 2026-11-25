//! Native DashScope text-embedding provider.
//!
//! DashScope's native API (used by the Qwen MaaS endpoints) is not
//! OpenAI-compatible: it posts to
//! `{base_url}/services/embeddings/text-embedding/text-embedding` with
//! `{"model":..., "input":{"texts":[...]}, "parameters":{"text_type":...}}`.
//! This adapter keeps that protocol inside the dashboard while exposing the
//! same [`EmbeddingProvider`] trait used by `kb-core`.

use std::time::Duration;

use async_trait::async_trait;
use kb_core::config::ApiEmbeddingConfig;
use kb_core::embedding::EmbeddingProvider;
use kb_core::error::{KbError, Result};
use serde::{Deserialize, Serialize};

/// DashScope accepts a bounded number of texts per call; stay conservative.
const MAX_BATCH_SIZE: usize = 10;
/// Maximum characters of an error body included in a message.
const MAX_ERROR_BODY_CHARS: usize = 500;
/// Retry attempts for transient failures.
const MAX_RETRIES: usize = 2;

/// DashScope native `/services/embeddings/text-embedding` provider.
pub struct DashScopeEmbedding {
    client: reqwest::Client,
    endpoint: String,
    api_key: String,
    model: String,
    dimension: usize,
    batch_size: usize,
}

impl DashScopeEmbedding {
    /// Builds a provider and probes the model dimension when it is unknown.
    pub async fn new(config: &ApiEmbeddingConfig, batch_size: usize) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|error| {
                KbError::Embedding(format!("创建 DashScope HTTP 客户端失败: {error}"))
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
            dimension: config.dimensions,
            batch_size: batch_size.clamp(1, MAX_BATCH_SIZE),
        };
        if provider.dimension == 0 {
            let probe = ["dimension probe".to_string()];
            let vectors = provider.request_batch(&probe, "document").await?;
            let dimension = vectors
                .first()
                .map(Vec::len)
                .filter(|dimension| *dimension > 0)
                .ok_or_else(|| KbError::Embedding("DashScope 返回了空向量".to_string()))?;
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
            parameters: DashScopeParameters { text_type },
        };
        let mut attempt = 0;
        loop {
            let mut request = self.client.post(&self.endpoint).json(&body);
            if !self.api_key.is_empty() {
                request = request.bearer_auth(&self.api_key);
            }
            let response = match request.send().await {
                Ok(response) => response,
                Err(error) => {
                    if (error.is_timeout() || error.is_connect()) && attempt < MAX_RETRIES {
                        attempt += 1;
                        sleep_backoff(attempt).await;
                        continue;
                    }
                    return Err(KbError::Embedding(format!("DashScope 请求失败: {error}")));
                }
            };
            let status = response.status();
            let raw = response
                .text()
                .await
                .map_err(|error| KbError::Embedding(format!("读取 DashScope 响应失败: {error}")))?;
            if !status.is_success() {
                if (status.as_u16() == 429 || status.is_server_error()) && attempt < MAX_RETRIES {
                    attempt += 1;
                    sleep_backoff(attempt).await;
                    continue;
                }
                return Err(KbError::Embedding(format!(
                    "DashScope API 返回 {status}: {}",
                    truncate_body(&raw)
                )));
            }
            let parsed: DashScopeResponse = serde_json::from_str(&raw).map_err(|error| {
                KbError::Embedding(format!(
                    "DashScope 响应解析失败: {error}; body: {}",
                    truncate_body(&raw)
                ))
            })?;
            if parsed.output.is_none() {
                if let Some(code) = parsed.code.as_deref().filter(|code| !code.is_empty()) {
                    return Err(KbError::Embedding(format!(
                        "DashScope API 错误 {code}: {}",
                        parsed.message.unwrap_or_default()
                    )));
                }
                return Err(KbError::Embedding(format!(
                    "DashScope 响应缺少 output: {}",
                    truncate_body(&raw)
                )));
            }
            let mut embeddings = parsed
                .output
                .map(|output| output.embeddings)
                .unwrap_or_default();
            embeddings.sort_by_key(|item| item.index);
            let vectors: Vec<Vec<f32>> =
                embeddings.into_iter().map(|item| item.embedding).collect();
            self.validate(texts.len(), &vectors)?;
            return Ok(vectors);
        }
    }

    /// Ensures the response matches the requested inputs and dimension.
    fn validate(&self, expected: usize, vectors: &[Vec<f32>]) -> Result<()> {
        if vectors.len() != expected {
            return Err(KbError::Embedding(format!(
                "DashScope 返回 {} 个向量，期望 {expected} 个",
                vectors.len()
            )));
        }
        if self.dimension > 0 {
            for (position, vector) in vectors.iter().enumerate() {
                if vector.len() != self.dimension {
                    return Err(KbError::Embedding(format!(
                        "DashScope 第 {position} 个向量为 {} 维，期望 {} 维",
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
            .ok_or_else(|| KbError::Embedding("DashScope 未返回查询向量".to_string()))
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
}

#[derive(Deserialize)]
struct DashScopeResponse {
    #[serde(default)]
    output: Option<DashScopeOutput>,
    #[serde(default)]
    code: Option<String>,
    #[serde(default)]
    message: Option<String>,
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
    #[serde(default)]
    index: usize,
}

async fn sleep_backoff(attempt: usize) {
    let delay = 300u64.saturating_mul(1u64 << attempt.min(4));
    tokio::time::sleep(Duration::from_millis(delay)).await;
}

fn truncate_body(body: &str) -> String {
    let body = body.trim();
    if body.chars().count() <= MAX_ERROR_BODY_CHARS {
        return body.to_string();
    }
    let mut truncated: String = body.chars().take(MAX_ERROR_BODY_CHARS).collect();
    truncated.push_str("...");
    truncated
}
