//! SiliconFlow/Cohere-compatible HTTP reranking.

use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::{
    validate_api_config, validate_ranked_documents, ApiRerankConfig, RankedDocument, RerankProvider,
};
use crate::error::{KbError, Result};

/// HTTP reranker that posts to a configured complete endpoint.
pub(super) struct ApiRerank {
    client: reqwest::Client,
    endpoint: String,
    model: String,
    api_key: String,
}

impl ApiRerank {
    pub(super) fn new(config: &ApiRerankConfig) -> Result<Self> {
        validate_api_config(config)?;
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(config.timeout_secs))
            .build()
            .map_err(|_| KbError::Rerank("failed to build rerank HTTP client".to_string()))?;

        Ok(Self {
            client,
            endpoint: config.endpoint.trim().to_string(),
            model: config.model.trim().to_string(),
            api_key: config.api_key.trim().to_string(),
        })
    }

    async fn post(&self, query: &str, documents: &[&str]) -> Result<Vec<RankedDocument>> {
        let request_body = RerankRequest {
            model: &self.model,
            query,
            documents,
            top_n: documents.len(),
        };
        let mut request = self.client.post(&self.endpoint).json(&request_body);
        if !self.api_key.is_empty() {
            request = request.bearer_auth(&self.api_key);
        }

        let response = request.send().await.map_err(|error| {
            let detail = if error.is_timeout() {
                "request timed out"
            } else if error.is_connect() {
                "could not connect"
            } else if error.is_body() {
                "response body could not be read"
            } else {
                "request failed"
            };
            KbError::Rerank(format!("rerank API {detail}"))
        })?;
        let status = response.status();
        if !status.is_success() {
            return Err(KbError::Rerank(format!(
                "rerank API returned HTTP status {status}"
            )));
        }

        let body = response.bytes().await.map_err(|_| {
            KbError::Rerank("rerank API response body could not be read".to_string())
        })?;
        let response: RerankResponse = serde_json::from_slice(&body).map_err(|error| {
            KbError::Rerank(format!("rerank API returned an invalid response: {error}"))
        })?;
        let ranked = response
            .results
            .into_iter()
            .map(|result| RankedDocument {
                index: result.index,
                score: result.relevance_score,
            })
            .collect();
        validate_ranked_documents(documents.len(), ranked)
    }
}

#[async_trait::async_trait]
impl RerankProvider for ApiRerank {
    async fn rerank(&self, query: &str, documents: &[&str]) -> Result<Vec<RankedDocument>> {
        if documents.is_empty() {
            return Ok(Vec::new());
        }
        self.post(query, documents).await
    }
}

#[derive(Serialize)]
struct RerankRequest<'a> {
    model: &'a str,
    query: &'a str,
    documents: &'a [&'a str],
    top_n: usize,
}

#[derive(Deserialize)]
struct RerankResponse {
    results: Vec<RerankResult>,
}

#[derive(Deserialize)]
struct RerankResult {
    index: usize,
    relevance_score: f32,
}

#[cfg(test)]
mod tests {
    use serde_json::json;
    use wiremock::matchers::{body_json, header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    use super::ApiRerank;
    use crate::error::KbError;
    use crate::rerank::{ApiRerankConfig, RankedDocument, RerankProvider};

    fn config(server: &MockServer, api_key: &str) -> ApiRerankConfig {
        ApiRerankConfig {
            endpoint: format!("{}/v1/rerank", server.uri()),
            model: "test-model".to_string(),
            api_key: api_key.to_string(),
            timeout_secs: 5,
        }
    }

    #[tokio::test]
    async fn matches_siliconflow_cohere_request_and_sorts_all_results() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/v1/rerank"))
            .and(header("Authorization", "Bearer sk-test"))
            .and(header("Content-Type", "application/json"))
            .and(body_json(json!({
                "model": "test-model",
                "query": "holiday policy",
                "documents": ["annual leave", "expenses", "security"],
                "top_n": 3
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "id": "request-id",
                "results": [
                    {"index": 2, "relevance_score": 0.2},
                    {"index": 0, "relevance_score": 0.9},
                    {"index": 1, "relevance_score": 0.4}
                ],
                "meta": {"billed_units": {"search_units": 1}}
            })))
            .expect(1)
            .mount(&server)
            .await;

        let provider = ApiRerank::new(&config(&server, "sk-test")).unwrap();
        let ranked = provider
            .rerank("holiday policy", &["annual leave", "expenses", "security"])
            .await
            .unwrap();

        assert_eq!(
            ranked,
            vec![
                RankedDocument {
                    index: 0,
                    score: 0.9,
                },
                RankedDocument {
                    index: 1,
                    score: 0.4,
                },
                RankedDocument {
                    index: 2,
                    score: 0.2,
                },
            ]
        );
    }

    #[tokio::test]
    async fn omits_authorization_for_an_empty_key() {
        let server = MockServer::start().await;
        Mock::given(method("POST"))
            .and(path("/rerank"))
            .and(body_json(json!({
                "model": "local-model",
                "query": "q",
                "documents": ["a", "b"],
                "top_n": 2
            })))
            .respond_with(ResponseTemplate::new(200).set_body_json(json!({
                "results": [
                    {"index": 1, "relevance_score": 0.4},
                    {"index": 0, "relevance_score": 0.8}
                ]
            })))
            .expect(1)
            .mount(&server)
            .await;

        let mut config = config(&server, "");
        config.endpoint = format!("{}/rerank", server.uri());
        config.model = "local-model".to_string();
        let provider = ApiRerank::new(&config).unwrap();
        provider.rerank("q", &["a", "b"]).await.unwrap();

        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 1);
        assert!(requests[0]
            .headers
            .get(reqwest::header::AUTHORIZATION)
            .is_none());
    }

    #[tokio::test]
    async fn rejects_incomplete_duplicate_and_out_of_range_results() {
        let cases = [
            (
                json!({
                    "results": [
                        {"index": 0, "relevance_score": 0.9}
                    ]
                }),
                "returned 1 results",
            ),
            (
                json!({
                    "results": [
                        {"index": 0, "relevance_score": 0.9},
                        {"index": 0, "relevance_score": 0.1}
                    ]
                }),
                "duplicate document index 0",
            ),
            (
                json!({
                    "results": [
                        {"index": 0, "relevance_score": 0.9},
                        {"index": 2, "relevance_score": 0.1}
                    ]
                }),
                "out-of-range document index 2",
            ),
        ];

        for (body, expected_message) in cases {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/rerank"))
                .respond_with(ResponseTemplate::new(200).set_body_json(body))
                .expect(1)
                .mount(&server)
                .await;

            let provider = ApiRerank::new(&config(&server, "sk-test")).unwrap();
            let error = provider.rerank("q", &["a", "b"]).await.unwrap_err();
            assert!(matches!(error, KbError::Rerank(_)));
            assert!(
                error.to_string().contains(expected_message),
                "unexpected error: {error}"
            );
        }
    }

    #[tokio::test]
    async fn rejects_malformed_json_and_missing_result_fields() {
        let malformed_bodies = [
            "{not-json".to_string(),
            json!({"results": [{"index": 0}]}).to_string(),
            json!({"not_results": []}).to_string(),
        ];

        for body in malformed_bodies {
            let server = MockServer::start().await;
            Mock::given(method("POST"))
                .and(path("/v1/rerank"))
                .respond_with(
                    ResponseTemplate::new(200)
                        .insert_header("Content-Type", "application/json")
                        .set_body_string(body),
                )
                .expect(1)
                .mount(&server)
                .await;

            let provider = ApiRerank::new(&config(&server, "sk-test")).unwrap();
            let error = provider.rerank("q", &["a"]).await.unwrap_err();
            assert!(matches!(error, KbError::Rerank(_)));
            assert!(error.to_string().contains("invalid response"));
        }
    }

    #[tokio::test]
    async fn upstream_errors_do_not_expose_endpoint_or_key() {
        let server = MockServer::start().await;
        let api_key = "sk-do-not-leak";
        Mock::given(method("POST"))
            .and(path("/v1/rerank"))
            .respond_with(
                ResponseTemplate::new(401)
                    .set_body_string(format!("provider echoed {api_key} for {}", server.uri())),
            )
            .expect(1)
            .mount(&server)
            .await;

        let provider = ApiRerank::new(&config(&server, api_key)).unwrap();
        let error = provider.rerank("q", &["a"]).await.unwrap_err();
        let message = error.to_string();

        assert!(matches!(error, KbError::Rerank(_)));
        assert!(message.contains("HTTP status 401"));
        assert!(!message.contains(api_key));
        assert!(!message.contains(server.uri().as_str()));
    }
}
