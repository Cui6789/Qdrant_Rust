//! Selects an upload parser and converts its result into the normal ingest input.

use std::path::Path;

use serde::{Deserialize, Serialize};

use crate::error::{KbError, Result};
use crate::models::{DocumentFormat, DocumentMetadata};
use crate::service::IngestInput;

use super::mineru::{self, MineruConfig, MineruMode, MineruParser};
use super::parser::decode_bytes;

#[derive(Clone, Default, Deserialize, Serialize)]
#[serde(default)]
pub struct ParsingConfig {
    pub provider: ParsingProvider,
    pub mineru: MineruConfig,
}

impl std::fmt::Debug for ParsingConfig {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("ParsingConfig")
            .field("provider", &self.provider)
            .finish_non_exhaustive()
    }
}

impl ParsingConfig {
    pub fn validate(&self) -> Result<()> {
        mineru::validate_config(&self.mineru)?;
        if self.provider == ParsingProvider::MineruCloud && self.mineru.api_key.trim().is_empty() {
            return Err(KbError::Config(
                "parsing.mineru.api_key is required for mineru_cloud".into(),
            ));
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum ParsingProvider {
    #[default]
    Native,
    MineruLocal,
    MineruCloud,
}

impl ParsingProvider {
    pub fn parse(value: &str) -> Result<Self> {
        match value {
            "native" => Ok(Self::Native),
            "mineru_local" => Ok(Self::MineruLocal),
            "mineru_cloud" => Ok(Self::MineruCloud),
            _ => Err(KbError::Validation(
                "parser must be native, mineru_local, or mineru_cloud".into(),
            )),
        }
    }
}

/// An uploaded file and the same metadata accepted by text ingestion.
pub struct UploadInput {
    pub filename: String,
    pub bytes: Vec<u8>,
    pub doc_id: Option<String>,
    pub format: Option<DocumentFormat>,
    pub encoding: Option<String>,
    pub parser: Option<ParsingProvider>,
    pub metadata: DocumentMetadata,
}

#[derive(Default)]
pub struct UploadParser {
    provider: ParsingProvider,
    mineru: Option<MineruParser>,
}

impl UploadParser {
    pub fn new(config: &ParsingConfig) -> Result<Self> {
        config.validate()?;
        Ok(Self {
            provider: config.provider,
            mineru: Some(MineruParser::new(&config.mineru)?),
        })
    }

    pub async fn prepare(&self, input: UploadInput) -> Result<IngestInput> {
        let binary = needs_document_parser(&input.filename);
        let provider = input.parser.unwrap_or_else(|| {
            if binary && input.format.is_none() {
                self.provider
            } else {
                ParsingProvider::Native
            }
        });

        let (text, format) = match provider {
            ParsingProvider::Native => {
                if binary && input.format.is_none() {
                    return Err(KbError::UnsupportedFormat(
                        "select a MinerU parser to upload this file type".into(),
                    ));
                }
                let format = input
                    .format
                    .unwrap_or_else(|| DocumentFormat::infer_from_filename(&input.filename));
                (
                    decode_bytes(&input.bytes, input.encoding.as_deref())?,
                    format,
                )
            }
            ParsingProvider::MineruLocal | ParsingProvider::MineruCloud => {
                if input.format.is_some() || input.encoding.is_some() {
                    return Err(KbError::Validation(
                        "format and encoding apply to the native parser".into(),
                    ));
                }
                let parser = self.mineru.as_ref().ok_or_else(|| {
                    KbError::Config("the MinerU parser requires an upload configuration".into())
                })?;
                let mode = if provider == ParsingProvider::MineruCloud {
                    MineruMode::Cloud
                } else {
                    MineruMode::Local
                };
                let markdown = parser.parse(mode, &input.filename, input.bytes).await?;
                (markdown, DocumentFormat::Markdown)
            }
        };

        Ok(IngestInput {
            doc_id: input.doc_id,
            text,
            format,
            metadata: input.metadata,
        })
    }
}

fn needs_document_parser(name: &str) -> bool {
    let extension = Path::new(name)
        .extension()
        .and_then(|ext| ext.to_str())
        .unwrap_or_default();
    matches!(
        extension.to_ascii_lowercase().as_str(),
        "pdf"
            | "doc"
            | "docx"
            | "ppt"
            | "pptx"
            | "xls"
            | "xlsx"
            | "png"
            | "jpg"
            | "jpeg"
            | "bmp"
            | "tif"
            | "tiff"
            | "webp"
            | "gif"
            | "jp2"
    )
}
