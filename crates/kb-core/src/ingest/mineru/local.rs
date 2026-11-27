use std::collections::BTreeMap;

use reqwest::multipart::{Form, Part};
use serde::Deserialize;
use tokio::time::Instant;

use super::common;
use super::{MineruParser, Result};

pub(super) async fn parse(
    parser: &MineruParser,
    name: &str,
    bytes: Vec<u8>,
    deadline: Instant,
) -> Result<String> {
    common::remaining(deadline, "local parsing")?;

    let file = Part::bytes(bytes)
        .file_name(name.to_owned())
        .mime_str("application/octet-stream")
        .map_err(|_| {
            crate::error::KbError::Parse("could not encode the MinerU upload".to_owned())
        })?;
    // MinerU 3.x names its single-file stream `files`; it normalizes the
    // filename server-side and returns `results.<stem>.md_content` JSON.
    let form = Form::new()
        .part("files", file)
        .text("parse_method", "auto")
        .text("return_md", "true")
        .text("return_middle_json", "false")
        .text("return_model_output", "false")
        .text("return_content_list", "false")
        .text("return_images", "false")
        .text("response_format_zip", "false");

    let response = tokio::time::timeout_at(
        deadline,
        parser
            .api_client
            .post(parser.local_url.clone())
            .multipart(form)
            .send(),
    )
    .await
    .map_err(|_| common::deadline_error("local parsing"))?
    .map_err(|error| common::request_error("local parsing", &error))?;

    let status = response.status();
    if !status.is_success() {
        return Err(common::status_error("local parser", status));
    }

    let response: LocalParseResponse = common::read_json_limited(
        response,
        common::MAX_LOCAL_JSON_BYTES,
        "local parser",
        deadline,
    )
    .await?;
    nonempty_markdown(response.into_markdown())
}

#[derive(Deserialize)]
struct LocalParseResponse {
    #[serde(default)]
    md_content: Option<String>,
    #[serde(default)]
    results: Option<BTreeMap<String, LocalDocumentResult>>,
}

impl LocalParseResponse {
    fn into_markdown(self) -> Option<String> {
        if let Some(markdown) = self.md_content {
            return Some(markdown);
        }
        let results = self.results?;
        let mut documents = results
            .into_values()
            .filter_map(|document| document.md_content);
        let markdown = documents.next()?;
        if documents.next().is_some() {
            return None;
        }
        Some(markdown)
    }
}

#[derive(Deserialize)]
struct LocalDocumentResult {
    #[serde(default)]
    md_content: Option<String>,
}

fn nonempty_markdown(markdown: Option<String>) -> Result<String> {
    let markdown = markdown.ok_or_else(|| {
        crate::error::KbError::ParserUnavailable(
            "MinerU local parser returned no Markdown".to_owned(),
        )
    })?;
    let markdown = markdown.trim();
    if markdown.is_empty() {
        return Err(crate::error::KbError::ParserUnavailable(
            "MinerU local parser returned empty Markdown".to_owned(),
        ));
    }
    Ok(markdown.to_owned())
}
