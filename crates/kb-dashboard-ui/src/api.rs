//! Typed fetch helpers for the dashboard API.

use gloo_net::http::{Request, Response};
use kb_dashboard_types::*;
use serde::de::DeserializeOwned;
use serde::Serialize;

/// A user-presentable API failure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiError {
    pub code: String,
    pub message: String,
}

impl ApiError {
    fn network(message: impl Into<String>) -> Self {
        Self {
            code: "NETWORK".to_string(),
            message: message.into(),
        }
    }
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(formatter, "{}", self.message)
    }
}

/// Same-origin API base. During `trunk serve` on port 8081 it targets the
/// dashboard binary on port 8090.
fn base() -> String {
    let Some(window) = web_sys::window() else {
        return "/api".to_string();
    };
    let location = window.location();
    let port = location.port().unwrap_or_default();
    if port.is_empty() || port == "8090" {
        "/api".to_string()
    } else {
        let host = location
            .hostname()
            .unwrap_or_else(|_| "127.0.0.1".to_string());
        format!("http://{host}:8090/api")
    }
}

fn url(path: &str) -> String {
    format!("{}{}", base(), path)
}

fn encode(value: &str) -> String {
    js_sys::encode_uri_component(value)
        .as_string()
        .unwrap_or_default()
}

async fn parse<T: DeserializeOwned>(response: Response) -> Result<T, ApiError> {
    let status = response.status();
    if response.ok() {
        return response
            .json::<T>()
            .await
            .map_err(|error| ApiError::network(format!("解析响应失败：{error}")));
    }
    let text = response.text().await.unwrap_or_default();
    if let Ok(body) = serde_json::from_str::<ApiErrorBody>(&text) {
        return Err(ApiError {
            code: body.error.code,
            message: body.error.message,
        });
    }
    Err(ApiError {
        code: format!("HTTP_{status}"),
        message: if text.trim().is_empty() {
            format!("请求失败（HTTP {status}）")
        } else {
            text
        },
    })
}

async fn get<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let response = Request::get(&url(path))
        .send()
        .await
        .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
    parse(response).await
}

async fn post<B: Serialize, T: DeserializeOwned>(path: &str, body: &B) -> Result<T, ApiError> {
    let request = Request::post(&url(path))
        .json(body)
        .map_err(|error| ApiError::network(format!("构造请求失败：{error}")))?;
    let response = request
        .send()
        .await
        .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
    parse(response).await
}

async fn delete<T: DeserializeOwned>(path: &str) -> Result<T, ApiError> {
    let response = Request::delete(&url(path))
        .send()
        .await
        .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
    parse(response).await
}

pub async fn overview() -> Result<OverviewResponse, ApiError> {
    get("/overview").await
}

pub async fn embedding_state() -> Result<EmbeddingConfigStateResponse, ApiError> {
    get("/embedding/config").await
}

pub async fn configure_embedding(
    request: &EmbeddingConfigRequest,
) -> Result<EmbeddingConfigResponse, ApiError> {
    post("/embedding/config", request).await
}

pub async fn configure_mock() -> Result<EmbeddingStatus, ApiError> {
    post(
        "/embedding/mock",
        &MockEmbeddingRequest {
            dimension: Some(512),
        },
    )
    .await
}

pub async fn clear_embedding() -> Result<EmbeddingStatus, ApiError> {
    delete("/embedding/config").await
}

pub async fn documents(
    collection: &str,
    query: Option<&str>,
    limit: u32,
) -> Result<DocumentListResponse, ApiError> {
    let mut path = format!(
        "/collections/{}/documents?limit={}",
        encode(collection),
        limit
    );
    if let Some(query) = query.filter(|value| !value.trim().is_empty()) {
        path.push_str("&q=");
        path.push_str(&encode(query.trim()));
    }
    get(&path).await
}

pub async fn document_detail(
    collection: &str,
    doc_id: &str,
) -> Result<DocumentDetailResponse, ApiError> {
    get(&format!(
        "/collections/{}/document?doc_id={}",
        encode(collection),
        encode(doc_id)
    ))
    .await
}

pub async fn delete_document(
    collection: &str,
    doc_id: &str,
) -> Result<DeleteDocumentResponse, ApiError> {
    delete(&format!(
        "/collections/{}/document?doc_id={}",
        encode(collection),
        encode(doc_id)
    ))
    .await
}

pub async fn search(
    collection: &str,
    query: &str,
    top_k: usize,
) -> Result<SearchResponse, ApiError> {
    post(
        &format!("/collections/{}/search", encode(collection)),
        &SearchRequest {
            query: query.to_string(),
            top_k: Some(top_k),
            score_threshold: None,
        },
    )
    .await
}

pub async fn start_pipeline(request: &PipelineRequest) -> Result<PipelineStartResponse, ApiError> {
    post("/pipeline/run", request).await
}

pub async fn pipeline_job(id: &str) -> Result<PipelineJob, ApiError> {
    get(&format!("/pipeline/jobs/{}", encode(id))).await
}
