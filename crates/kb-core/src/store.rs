//! Qdrant access layer.
//!
//! [`QdrantStore`] is a thin, error-mapped wrapper around `qdrant-client`. It
//! owns no business logic: identity, reconciliation and metadata rules live in
//! [`crate::service`]. Every write is issued with `wait(true)` so a returned
//! success means Qdrant has applied the change.

use std::collections::HashMap;
use std::time::Duration;

use qdrant_client::qdrant::{
    point_id::PointIdOptions, vectors_config, CountPointsBuilder, CreateCollectionBuilder,
    DeleteCollectionBuilder, DeletePointsBuilder, Distance, PayloadIncludeSelector, PointId,
    QueryPointsBuilder, ScrollPointsBuilder, SetPayloadPointsBuilder, UpsertPointsBuilder,
    VectorParamsBuilder,
};
use qdrant_client::{Qdrant, QdrantError};

// Re-export the Qdrant types that appear in this module's public API, so a
// downstream crate can build filters and inspect raw search results without
// adding `qdrant-client` as a direct dependency.
pub use qdrant_client::qdrant::r#match::MatchValue;
pub use qdrant_client::qdrant::{Condition, Filter, PointStruct, ScoredPoint};
pub use qdrant_client::Payload;

use crate::config::QdrantConfig;
use crate::error::{KbError, Result};
use crate::models::CollectionInfo;

/// Readiness polling interval used by [`QdrantStore::wait_ready`].
const READY_POLL: Duration = Duration::from_millis(500);

/// A stored point's id and payload, as consumed by read-only dashboards.
#[derive(Debug, Clone)]
pub struct StoredPoint {
    pub id: String,
    pub payload: serde_json::Map<String, serde_json::Value>,
}

/// Payload selection for the shared Qdrant scroll path.
#[derive(Debug, Clone, Copy)]
pub(crate) enum ScrollPayloadSelection {
    None,
    All,
    Include(&'static [&'static str]),
}

/// One stored-point page plus Qdrant's cursor for the following page.
#[derive(Debug, Clone)]
pub(crate) struct StoredPointPage {
    pub points: Vec<StoredPoint>,
    pub next_cursor: Option<String>,
}

/// Connection to a Qdrant server.
#[derive(Clone)]
pub struct QdrantStore {
    client: Qdrant,
    url: String,
}

impl QdrantStore {
    /// Builds a client from configuration. The API key is only sent when set.
    pub fn new(config: &QdrantConfig) -> Result<Self> {
        let mut builder = Qdrant::from_url(&config.url)
            .timeout(Duration::from_secs(config.timeout_secs))
            .skip_compatibility_check();
        if !config.api_key.trim().is_empty() {
            builder = builder.api_key(config.api_key.clone());
        }
        let client = builder.build().map_err(|error| map_error(None, error))?;
        Ok(Self {
            client,
            url: config.url.clone(),
        })
    }

    /// Polls Qdrant until it answers or `retry` elapses.
    pub async fn wait_ready(&self, retry: Duration) -> Result<()> {
        let deadline = std::time::Instant::now() + retry;
        loop {
            match self.client.list_collections().await {
                Ok(_) => return Ok(()),
                Err(error) => {
                    if std::time::Instant::now() >= deadline {
                        return Err(KbError::Qdrant(format!(
                            "qdrant at {} did not become ready within {:?}: {error}",
                            self.url, retry
                        )));
                    }
                }
            }
            tokio::time::sleep(READY_POLL).await;
        }
    }

    /// Creates a cosine-distance collection with `dim`-dimensional vectors.
    pub async fn create_collection(&self, name: &str, dim: u64) -> Result<()> {
        self.client
            .create_collection(
                CreateCollectionBuilder::new(name)
                    .vectors_config(VectorParamsBuilder::new(dim, Distance::Cosine)),
            )
            .await
            .map(|_| ())
            .map_err(|error| map_error(Some(name), error))
    }

    /// Deletes a collection.
    pub async fn delete_collection(&self, name: &str) -> Result<()> {
        self.client
            .delete_collection(DeleteCollectionBuilder::new(name))
            .await
            .map(|_| ())
            .map_err(|error| map_error(Some(name), error))
    }

    /// Returns whether a collection exists.
    pub async fn collection_exists(&self, name: &str) -> Result<bool> {
        self.client
            .collection_exists(name)
            .await
            .map_err(|error| map_error(Some(name), error))
    }

    /// Lists collections with their real point counts and vector sizes.
    pub async fn list_collections(&self) -> Result<Vec<CollectionInfo>> {
        let response = self
            .client
            .list_collections()
            .await
            .map_err(|error| map_error(None, error))?;
        let mut collections = Vec::with_capacity(response.collections.len());
        for description in response.collections {
            collections.push(self.collection_info(&description.name).await?);
        }
        Ok(collections)
    }

    /// Reads one collection's point count and vector size from Qdrant.
    pub async fn collection_info(&self, name: &str) -> Result<CollectionInfo> {
        let response = self
            .client
            .collection_info(name)
            .await
            .map_err(|error| map_error(Some(name), error))?;
        let info = response
            .result
            .ok_or_else(|| KbError::CollectionNotFound(name.to_string()))?;
        let vector_size = info
            .config
            .as_ref()
            .and_then(|config| config.params.as_ref())
            .and_then(|params| params.vectors_config.as_ref())
            .and_then(|config| match config.config.as_ref() {
                Some(vectors_config::Config::Params(params)) => Some(params.size),
                Some(vectors_config::Config::ParamsMap(_)) | None => None,
            })
            .unwrap_or(0);
        Ok(CollectionInfo {
            name: name.to_string(),
            points_count: info.points_count.unwrap_or(0),
            vector_size,
        })
    }

    /// Upserts points and waits for Qdrant to apply them.
    pub async fn upsert(&self, collection: &str, points: Vec<PointStruct>) -> Result<()> {
        self.client
            .upsert_points(UpsertPointsBuilder::new(collection, points).wait(true))
            .await
            .map(|_| ())
            .map_err(|error| map_error(Some(collection), error))
    }

    /// Deletes every point matching `filter`.
    pub async fn delete_by_filter(&self, collection: &str, filter: Filter) -> Result<()> {
        self.client
            .delete_points(
                DeletePointsBuilder::new(collection)
                    .points(filter)
                    .wait(true),
            )
            .await
            .map(|_| ())
            .map_err(|error| map_error(Some(collection), error))
    }

    /// Counts points matching `filter`, or all points when no filter is given.
    pub async fn count(&self, collection: &str, filter: Option<Filter>) -> Result<u64> {
        let mut builder = CountPointsBuilder::new(collection).exact(true);
        if let Some(filter) = filter {
            builder = builder.filter(filter);
        }
        let response = self
            .client
            .count(builder)
            .await
            .map_err(|error| map_error(Some(collection), error))?;
        Ok(response.result.map(|result| result.count).unwrap_or(0))
    }

    /// Scrolls matching points and returns their payloads as JSON objects.
    pub async fn scroll_payloads(
        &self,
        collection: &str,
        filter: Filter,
        limit: u32,
    ) -> Result<Vec<serde_json::Map<String, serde_json::Value>>> {
        let page = self
            .scroll_page(
                collection,
                Some(filter),
                limit,
                None,
                ScrollPayloadSelection::All,
            )
            .await?;
        Ok(page.points.into_iter().map(|point| point.payload).collect())
    }

    /// Scrolls matching points and returns their ids together with payloads.
    ///
    /// This is the read-only counterpart of [`Self::upsert`] used by the
    /// dashboard to enumerate documents without knowing point ids in advance.
    pub async fn scroll_points(
        &self,
        collection: &str,
        filter: Option<Filter>,
        limit: u32,
    ) -> Result<Vec<StoredPoint>> {
        Ok(self
            .scroll_page(collection, filter, limit, None, ScrollPayloadSelection::All)
            .await?
            .points)
    }

    /// Scrolls matching points and returns their UUID point ids.
    pub async fn scroll_ids(
        &self,
        collection: &str,
        filter: Filter,
        limit: u32,
    ) -> Result<Vec<uuid::Uuid>> {
        let page = self
            .scroll_page(
                collection,
                Some(filter),
                limit,
                None,
                ScrollPayloadSelection::None,
            )
            .await?;
        let mut ids = Vec::with_capacity(page.points.len());
        for point in page.points {
            ids.push(uuid::Uuid::parse_str(&point.id).map_err(|error| {
                KbError::Qdrant(format!("invalid UUID point id '{}': {error}", point.id))
            })?);
        }
        Ok(ids)
    }

    /// Shared cursor-based scroll used by the public browsing primitives.
    pub(crate) async fn scroll_page(
        &self,
        collection: &str,
        filter: Option<Filter>,
        limit: u32,
        cursor: Option<&str>,
        payload: ScrollPayloadSelection,
    ) -> Result<StoredPointPage> {
        let mut builder = ScrollPointsBuilder::new(collection)
            .limit(limit)
            .with_vectors(false);
        builder = match payload {
            ScrollPayloadSelection::None => builder.with_payload(false),
            ScrollPayloadSelection::All => builder.with_payload(true),
            ScrollPayloadSelection::Include(fields) => {
                builder.with_payload(PayloadIncludeSelector {
                    fields: fields.iter().map(|field| (*field).to_string()).collect(),
                })
            }
        };
        if let Some(filter) = filter {
            builder = builder.filter(filter);
        }
        if let Some(cursor) = cursor {
            builder = builder.offset(cursor_to_point_id(cursor)?);
        }
        let response = self
            .client
            .scroll(builder)
            .await
            .map_err(|error| map_error(Some(collection), error))?;
        let next_cursor = response
            .next_page_offset
            .and_then(|point_id| point_id_string(point_id.point_id_options));
        let points = response
            .result
            .into_iter()
            .map(|point| StoredPoint {
                id: point
                    .id
                    .and_then(|point_id| point_id_string(point_id.point_id_options))
                    .unwrap_or_default(),
                payload: point
                    .payload
                    .into_iter()
                    .map(|(key, value)| (key, serde_json::Value::from(value)))
                    .collect(),
            })
            .collect();
        Ok(StoredPointPage {
            points,
            next_cursor,
        })
    }

    /// Overwrites the given payload keys on every point matching `filter`.
    pub async fn set_metadata_payload(
        &self,
        collection: &str,
        filter: Filter,
        payload: serde_json::Map<String, serde_json::Value>,
    ) -> Result<()> {
        let payload: HashMap<String, qdrant_client::qdrant::Value> = payload
            .into_iter()
            .map(|(key, value)| (key, value.into()))
            .collect();
        self.client
            .set_payload(
                SetPayloadPointsBuilder::new(collection, payload)
                    .points_selector(filter)
                    .wait(true),
            )
            .await
            .map(|_| ())
            .map_err(|error| map_error(Some(collection), error))
    }

    /// Runs a vector query and returns the scored points.
    pub async fn search(
        &self,
        collection: &str,
        vector: Vec<f32>,
        limit: u64,
        threshold: Option<f32>,
        filter: Option<Filter>,
    ) -> Result<Vec<ScoredPoint>> {
        let mut builder = QueryPointsBuilder::new(collection)
            .query(vector)
            .limit(limit)
            .with_payload(true);
        if let Some(threshold) = threshold {
            builder = builder.score_threshold(threshold);
        }
        if let Some(filter) = filter {
            builder = builder.filter(filter);
        }
        let response = self
            .client
            .query(builder)
            .await
            .map_err(|error| map_error(Some(collection), error))?;
        Ok(response.result)
    }
}

fn cursor_to_point_id(cursor: &str) -> Result<PointId> {
    let cursor = cursor.trim();
    if cursor.is_empty() {
        return Err(KbError::Validation(
            "pagination cursor must not be empty".to_string(),
        ));
    }
    if let Ok(value) = cursor.parse::<u64>() {
        return Ok(PointId::from(value));
    }
    let value = uuid::Uuid::parse_str(cursor).map_err(|_| {
        KbError::Validation("pagination cursor must be a Qdrant UUID or u64 point id".to_string())
    })?;
    Ok(PointId::from(value.to_string()))
}

fn point_id_string(value: Option<PointIdOptions>) -> Option<String> {
    match value {
        Some(PointIdOptions::Uuid(value)) => Some(value),
        Some(PointIdOptions::Num(value)) => Some(value.to_string()),
        None => None,
    }
}

/// Maps a Qdrant client error into the domain error type.
fn map_error(collection: Option<&str>, error: QdrantError) -> KbError {
    let message = error.to_string();
    let lowered = message.to_ascii_lowercase();
    if lowered.contains("doesn't exist") || lowered.contains("not found") {
        KbError::CollectionNotFound(collection.unwrap_or("unknown").to_string())
    } else if lowered.contains("already exists") {
        KbError::CollectionExists(collection.unwrap_or("unknown").to_string())
    } else if lowered.contains("dimension") {
        KbError::DimensionMismatch(message)
    } else {
        KbError::Qdrant(message)
    }
}
