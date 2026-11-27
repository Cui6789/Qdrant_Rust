//! Multipart parsing for the knowledge base upload route.

use axum::extract::multipart::MultipartRejection;
use axum::extract::{Multipart, Path, State};
use axum::Json;
use kb_core::error::KbError;
use kb_core::ingest::upload::{ParsingProvider, UploadInput};
use kb_core::models::{DocumentFormat, DocumentMetadata, IngestResult};

use super::handlers::{rejection, ApiResult};
use crate::state::HttpState;

pub async fn upload_document(
    State(state): State<HttpState>,
    Path(collection): Path<String>,
    multipart: Result<Multipart, MultipartRejection>,
) -> ApiResult<Json<IngestResult>> {
    let mut multipart = multipart.map_err(rejection)?;
    let mut file = None;
    let mut filename = String::new();
    let mut doc_id = None;
    let mut format = None;
    let mut encoding = None;
    let mut parser = None;
    let mut metadata = DocumentMetadata::default();

    while let Some(field) = multipart.next_field().await.map_err(rejection)? {
        match field.name().unwrap_or_default() {
            "file" => {
                if file.is_some() {
                    return Err(KbError::Validation("upload accepts one file".into()).into());
                }
                filename = field.file_name().unwrap_or_default().to_string();
                file = Some(field.bytes().await.map_err(rejection)?.to_vec());
            }
            "doc_id" => doc_id = Some(field.text().await.map_err(rejection)?),
            "source" => metadata.source = Some(field.text().await.map_err(rejection)?),
            "title" => metadata.title = Some(field.text().await.map_err(rejection)?),
            "tags" => metadata.tags.extend(
                field
                    .text()
                    .await
                    .map_err(rejection)?
                    .split(',')
                    .map(str::trim)
                    .filter(|tag| !tag.is_empty())
                    .map(str::to_owned),
            ),
            "format" => format = Some(field.text().await.map_err(rejection)?),
            "encoding" => encoding = Some(field.text().await.map_err(rejection)?),
            "parser" => parser = Some(field.text().await.map_err(rejection)?),
            _ => {}
        }
    }

    let bytes =
        file.ok_or_else(|| KbError::Validation("multipart field 'file' is required".into()))?;
    let input = UploadInput {
        filename,
        bytes,
        doc_id,
        format: format.as_deref().map(DocumentFormat::parse).transpose()?,
        encoding,
        parser: parser.as_deref().map(ParsingProvider::parse).transpose()?,
        metadata,
    };
    Ok(Json(
        state.service().ingest_upload(&collection, input).await?,
    ))
}
