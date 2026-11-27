//! Typed client for the dashboard API.
//!
//! [`DashboardApp`](crate::DashboardApp) provides an [`ApiClient`] through Yew
//! context. A section used on its own falls back to [`ApiClient::default`],
//! which preserves the original same-origin behavior.

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

/// Base URL of the dashboard API, including its `/api` suffix.
///
/// The default client uses the current origin, including during `trunk serve`
/// where Trunk proxies `/api` to the workbench listener.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApiClient {
    base_url: String,
}

impl Default for ApiClient {
    fn default() -> Self {
        Self {
            base_url: "/api".to_string(),
        }
    }
}

impl ApiClient {
    /// Creates a client for an explicit API root such as
    /// `http://127.0.0.1:8090/api`. A trailing slash is ignored.
    pub fn new(base_url: impl Into<String>) -> Self {
        let trimmed = base_url.into().trim().trim_end_matches('/').to_string();
        if trimmed.is_empty() {
            return Self::default();
        }
        Self { base_url: trimmed }
    }

    /// Returns the configured API root, including `/api`.
    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base_url, path)
    }

    async fn get<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = Request::get(&self.url(path))
            .send()
            .await
            .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
        parse(response).await
    }

    async fn post<B: Serialize, T: DeserializeOwned>(
        &self,
        path: &str,
        body: &B,
    ) -> Result<T, ApiError> {
        let request = Request::post(&self.url(path))
            .json(body)
            .map_err(|error| ApiError::network(format!("构造请求失败：{error}")))?;
        let response = request
            .send()
            .await
            .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
        parse(response).await
    }

    async fn delete<T: DeserializeOwned>(&self, path: &str) -> Result<T, ApiError> {
        let response = Request::delete(&self.url(path))
            .send()
            .await
            .map_err(|error| ApiError::network(format!("网络请求失败：{error}")))?;
        parse(response).await
    }

    /// `GET /api/overview`.
    pub async fn overview(&self) -> Result<OverviewResponse, ApiError> {
        self.get("/overview").await
    }

    /// `GET /api/collections/{collection}/documents`.
    pub async fn documents(
        &self,
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
        self.get(&path).await
    }

    /// `GET /api/collections/{collection}/document`.
    pub async fn document_detail(
        &self,
        collection: &str,
        doc_id: &str,
    ) -> Result<DocumentDetailResponse, ApiError> {
        self.get(&format!(
            "/collections/{}/document?doc_id={}",
            encode(collection),
            encode(doc_id)
        ))
        .await
    }

    /// `DELETE /api/collections/{collection}/document`.
    pub async fn delete_document(
        &self,
        collection: &str,
        doc_id: &str,
    ) -> Result<DeleteDocumentResponse, ApiError> {
        self.delete(&format!(
            "/collections/{}/document?doc_id={}",
            encode(collection),
            encode(doc_id)
        ))
        .await
    }

    /// `POST /api/collections/{collection}/search`.
    pub async fn search(
        &self,
        collection: &str,
        query: &str,
        top_k: usize,
    ) -> Result<SearchResponse, ApiError> {
        self.post(
            &format!("/collections/{}/search", encode(collection)),
            &SearchRequest {
                query: query.to_string(),
                top_k: Some(top_k),
                score_threshold: None,
            },
        )
        .await
    }

    /// `POST /api/pipeline/run`.
    pub async fn start_pipeline(
        &self,
        request: &PipelineRequest,
    ) -> Result<PipelineStartResponse, ApiError> {
        self.post("/pipeline/run", request).await
    }

    /// `GET /api/pipeline/jobs/{id}`.
    pub async fn pipeline_job(&self, id: &str) -> Result<PipelineJob, ApiError> {
        self.get(&format!("/pipeline/jobs/{}", encode(id))).await
    }
}

/// Same-origin API root. During `trunk serve` on port 8081 it targets the
/// dashboard binary on port 8090.
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
