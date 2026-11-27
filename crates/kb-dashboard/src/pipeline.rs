//! Background ingestion jobs that stream stage-by-stage progress to the UI.

use std::sync::{Arc, Mutex};
use std::time::Instant;

use chrono::{SecondsFormat, Utc};
use kb_core::service::{IngestEvent, IngestInput, KnowledgeBaseService};
use kb_dashboard_types::{JobStatus, PipelineChunkPreview, PipelineEvent, PipelineResult};
use serde_json::json;

use crate::state::AppState;

/// Everything needed to start one ingestion job.
pub struct PreparedPipeline {
    pub job_id: String,
    pub collection: String,
    pub input: IngestInput,
    pub points_before: u64,
    pub service: Arc<KnowledgeBaseService>,
    pub started: Instant,
}

/// Accumulated artifacts the UI shows at the end of a run.
#[derive(Debug, Clone, Default)]
struct Artifacts {
    chunk_previews: Vec<PipelineChunkPreview>,
    vector_preview: Vec<f32>,
    dimension: usize,
    vector_norm: f32,
}

/// Starts the ingestion task in the background.
pub fn spawn(state: AppState, prepared: PreparedPipeline) {
    tokio::spawn(run(state, prepared));
}

/// Emits one timestamped event into the job log.
fn emit(
    state: &AppState,
    job_id: &str,
    started: Instant,
    stage: &str,
    message: impl Into<String>,
    data: serde_json::Value,
) {
    let now = Utc::now();
    let elapsed_ms = started.elapsed().as_millis() as u64;
    let message = message.into();
    let mut jobs = lock_jobs(state);
    jobs.update(job_id, |job| {
        job.events.push(PipelineEvent {
            seq: job.events.len() as u64 + 1,
            at: now.to_rfc3339_opts(SecondsFormat::Millis, true),
            elapsed_ms,
            stage: stage.to_string(),
            message,
            data,
        });
    });
}

/// Locks the job registry, recovering from a poisoned mutex.
fn lock_jobs(state: &AppState) -> std::sync::MutexGuard<'_, crate::state::JobStore> {
    state
        .jobs
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Executes the ingestion pipeline and records every stage transition.
async fn run(state: AppState, prepared: PreparedPipeline) {
    let job_id = prepared.job_id.clone();
    let collection = prepared.collection.clone();
    let started = prepared.started;

    {
        let mut jobs = lock_jobs(&state);
        jobs.update(&job_id, |job| job.status = JobStatus::Running);
    }

    let characters = prepared.input.text.chars().count();
    emit(
        &state,
        &job_id,
        started,
        "input",
        format!("已接收 {characters} 个字符"),
        json!({
            "characters": characters,
            "format": prepared.input.format,
            "doc_id": prepared.input.doc_id,
            "collection": collection,
        }),
    );

    let artifacts = Arc::new(Mutex::new(Artifacts::default()));

    let observer = {
        let state = state.clone();
        let job_id = job_id.clone();
        let artifacts = artifacts.clone();
        move |event: IngestEvent| match event {
            IngestEvent::Parsed { characters } => emit(
                &state,
                &job_id,
                started,
                "parse",
                format!("解析完成，共 {characters} 字符"),
                json!({ "characters": characters }),
            ),
            IngestEvent::Chunked { total, previews } => {
                let previews: Vec<PipelineChunkPreview> = previews
                    .into_iter()
                    .map(|preview| PipelineChunkPreview {
                        index: preview.index,
                        characters: preview.characters,
                        heading_path: preview.heading_path,
                        preview: preview.preview,
                    })
                    .collect();
                if let Ok(mut artifacts) = artifacts.lock() {
                    artifacts.chunk_previews = previews.clone();
                }
                emit(
                    &state,
                    &job_id,
                    started,
                    "chunk",
                    format!("切分为 {total} 个块"),
                    json!({ "total": total, "previews": previews }),
                );
            }
            IngestEvent::Existing { chunks } => emit(
                &state,
                &job_id,
                started,
                "reconcile",
                "内容与元数据均未变化，跳过嵌入与写入",
                json!({ "chunks": chunks }),
            ),
            IngestEvent::MetadataUpdated { chunks } => emit(
                &state,
                &job_id,
                started,
                "reconcile",
                "仅元数据变化，已原地更新 payload",
                json!({ "chunks": chunks }),
            ),
            IngestEvent::EmbeddingStarted {
                chunks,
                model,
                dimension,
            } => emit(
                &state,
                &job_id,
                started,
                "embed",
                format!("调用嵌入模型 {model}（{dimension} 维）"),
                json!({ "chunks": chunks, "model": model, "dimension": dimension }),
            ),
            IngestEvent::EmbeddingFinished {
                chunks,
                dimension,
                preview,
                norm,
            } => {
                if let Ok(mut artifacts) = artifacts.lock() {
                    artifacts.vector_preview = preview.clone();
                    artifacts.dimension = dimension;
                    artifacts.vector_norm = norm;
                }
                emit(
                    &state,
                    &job_id,
                    started,
                    "embed",
                    format!("嵌入完成：{chunks} 个向量 · {dimension} 维"),
                    json!({
                        "chunks": chunks,
                        "dimension": dimension,
                        "preview": preview,
                        "norm": norm,
                    }),
                );
            }
            IngestEvent::Writing { points, batches } => emit(
                &state,
                &job_id,
                started,
                "write",
                format!("写入 Qdrant：{points} 个点，分 {batches} 批"),
                json!({ "points": points, "batches": batches }),
            ),
            IngestEvent::Written { points } => emit(
                &state,
                &job_id,
                started,
                "write",
                format!("{points} 个点已写入并通过确认"),
                json!({ "points": points }),
            ),
            IngestEvent::Cleaned => emit(
                &state,
                &job_id,
                started,
                "cleanup",
                "旧版本残留点已清理",
                json!({}),
            ),
        }
    };

    let outcome = prepared
        .service
        .ingest_document_observed(&collection, prepared.input, &observer)
        .await;

    match outcome {
        Ok(result) => {
            let points_after = state
                .store
                .count(&collection, None)
                .await
                .unwrap_or(prepared.points_before);
            let (chunk_previews, vector_preview, mut dimension, vector_norm) = {
                let artifacts = artifacts
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                (
                    artifacts.chunk_previews.clone(),
                    artifacts.vector_preview.clone(),
                    artifacts.dimension,
                    artifacts.vector_norm,
                )
            };
            if dimension == 0 {
                dimension = prepared.service.embedder().dimension();
            }
            let duration_ms = started.elapsed().as_millis() as u64;
            let pipeline_result = PipelineResult {
                doc_id: result.doc_id,
                chunks: result.chunks,
                skipped: result.skipped,
                metadata_updated: result.metadata_updated,
                content_hash: result.content_hash,
                embedding_model: result.embedding_model,
                points_before: prepared.points_before,
                points_after,
                duration_ms,
                chunk_previews,
                vector_preview,
                dimension,
                vector_norm,
            };
            let message = if pipeline_result.skipped {
                "内容未变化，本次未消耗嵌入调用".to_string()
            } else if pipeline_result.metadata_updated {
                "元数据已更新，向量保持不变".to_string()
            } else {
                format!(
                    "入库完成：{} 个块 · {} ms",
                    pipeline_result.chunks, pipeline_result.duration_ms
                )
            };
            emit(
                &state,
                &job_id,
                started,
                "done",
                message,
                serde_json::to_value(&pipeline_result).unwrap_or(serde_json::Value::Null),
            );
            let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            lock_jobs(&state).finish_success(&job_id, pipeline_result, finished_at);
        }
        Err(error) => {
            let message = error.to_string();
            emit(
                &state,
                &job_id,
                started,
                "error",
                message.clone(),
                json!({ "message": message }),
            );
            tracing::warn!(job_id = %job_id, collection = %collection, error = %message, "ingestion job failed");
            let finished_at = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
            lock_jobs(&state).finish_failure(&job_id, message, finished_at);
        }
    }
}
