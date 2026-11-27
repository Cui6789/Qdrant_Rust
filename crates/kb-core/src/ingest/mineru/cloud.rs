use reqwest::{Client, StatusCode, Url};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use super::common;
use super::{cloud_model_for_input, MineruParser, Result};
use crate::error::KbError;

pub(super) async fn parse(
    parser: &MineruParser,
    name: &str,
    bytes: Vec<u8>,
    deadline: Instant,
) -> Result<String> {
    if parser.api_key.is_empty() {
        return Err(KbError::Config(
            "mineru.api_key is required for cloud mode".to_owned(),
        ));
    }
    let model_version = cloud_model_for_input(&parser.model_version, name)?;

    let create_url = endpoint(
        &parser.cloud_base,
        &["file-urls", "batch"],
        "could not construct the cloud create endpoint",
    )?;
    let request = CloudCreateRequest {
        files: vec![CloudCreateFile { name }],
        model_version,
    };
    let response = tokio::time::timeout_at(
        deadline,
        parser
            .api_client
            .post(create_url)
            .bearer_auth(&parser.api_key)
            .header(reqwest::header::ACCEPT, "application/json")
            .json(&request)
            .send(),
    )
    .await
    .map_err(|_| common::deadline_error("cloud task creation"))?
    .map_err(|error| common::request_error("cloud task creation", &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(common::status_error("cloud API", status));
    }
    let response: CloudResponse<CloudCreateData> = common::read_json_limited(
        response,
        common::MAX_API_JSON_BYTES,
        "cloud task creation",
        deadline,
    )
    .await?;
    let data = response.into_data("cloud task creation")?;
    validate_batch_id(&data.batch_id)?;
    let upload_url = match data.file_urls.as_slice() {
        [upload_url] if !upload_url.is_empty() => upload_url.clone(),
        _ => {
            return Err(common::invalid_upstream(
                "the API returned an unexpected number of upload URLs",
            ));
        }
    };

    upload(&parser.transfer_client, &upload_url, bytes, deadline).await?;
    let zip_url = poll(parser, &data.batch_id, name, deadline).await?;
    let response = tokio::time::timeout_at(
        deadline,
        parser
            .transfer_client
            .get(common::validate_transfer_url(&zip_url)?)
            .send(),
    )
    .await
    .map_err(|_| common::deadline_error("cloud result download"))?
    .map_err(|error| common::request_error("cloud result download", &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(common::status_error("cloud transfer", status));
    }

    common::read_markdown_zip(response, "cloud result", Some("full.md"), deadline).await
}

#[derive(Serialize)]
struct CloudCreateRequest<'a> {
    files: Vec<CloudCreateFile<'a>>,
    model_version: &'a str,
}

#[derive(Serialize)]
struct CloudCreateFile<'a> {
    name: &'a str,
}

#[derive(Deserialize)]
struct CloudResponse<T> {
    code: i64,
    #[serde(default)]
    data: Option<T>,
}

impl<T> CloudResponse<T> {
    fn into_data(self, stage: &str) -> Result<T> {
        if self.code != 0 {
            return Err(KbError::ParserUnavailable(format!(
                "MinerU {stage} rejected the request with code {}",
                self.code
            )));
        }
        self.data.ok_or_else(|| {
            KbError::ParserUnavailable(format!("MinerU {stage} returned no result data"))
        })
    }
}

#[derive(Default, Deserialize)]
struct CloudCreateData {
    batch_id: String,
    #[serde(default)]
    file_urls: Vec<String>,
}

#[derive(Default, Deserialize)]
struct CloudPollData {
    batch_id: String,
    #[serde(default)]
    extract_result: Vec<CloudExtractResult>,
}

#[derive(Deserialize)]
struct CloudExtractResult {
    #[serde(default)]
    file_name: Option<String>,
    #[serde(default)]
    state: String,
    #[serde(default)]
    full_zip_url: Option<String>,
}

async fn upload(client: &Client, url: &str, bytes: Vec<u8>, deadline: Instant) -> Result<()> {
    let url = common::validate_transfer_url(url)?;
    // The presigned storage stage must not receive the MinerU bearer token or
    // an inferred Content-Type; the official service requires neither.
    let response = tokio::time::timeout_at(deadline, client.put(url).body(bytes).send())
        .await
        .map_err(|_| common::deadline_error("cloud upload"))?
        .map_err(|error| common::request_error("cloud upload", &error))?;
    let status = response.status();
    if !status.is_success() {
        return Err(common::status_error("cloud upload", status));
    }
    Ok(())
}

async fn poll(
    parser: &MineruParser,
    batch_id: &str,
    expected_name: &str,
    deadline: Instant,
) -> Result<String> {
    loop {
        common::remaining(deadline, "cloud polling")?;
        let poll_url = endpoint(
            &parser.cloud_base,
            &["extract-results", "batch", batch_id],
            "could not construct the cloud poll endpoint",
        )?;
        let response = tokio::time::timeout_at(
            deadline,
            parser
                .api_client
                .get(poll_url)
                .bearer_auth(&parser.api_key)
                .header(reqwest::header::ACCEPT, "application/json")
                .send(),
        )
        .await
        .map_err(|_| common::deadline_error("cloud polling"))?
        .map_err(|error| common::request_error("cloud polling", &error))?;
        let status = response.status();
        let retry_after = common::retry_after(&response);
        if is_transient(status) {
            common::sleep_until(
                deadline,
                retry_after.unwrap_or(common::POLL_INTERVAL),
                "cloud polling",
            )
            .await?;
            continue;
        }
        if !status.is_success() {
            return Err(common::status_error("cloud API", status));
        }

        let response: CloudResponse<CloudPollData> = common::read_json_limited(
            response,
            common::MAX_API_JSON_BYTES,
            "cloud polling",
            deadline,
        )
        .await?;
        let data = response.into_data("cloud polling")?;
        if data.batch_id != batch_id {
            return Err(common::invalid_upstream(
                "the API returned an unexpected batch ID",
            ));
        }
        let result = select_result(&data.extract_result, expected_name)?;

        match result.state.to_ascii_lowercase().as_str() {
            "waiting-file" | "pending" | "running" | "converting" => {}
            "done" => {
                return result.full_zip_url.clone().ok_or_else(|| {
                    common::invalid_upstream("the completed task has no result archive")
                });
            }
            "failed" => return Err(common::invalid_upstream("the parse task failed")),
            _ => {
                return Err(common::invalid_upstream(
                    "the API returned an unknown parse state",
                ));
            }
        }
        common::sleep_until(deadline, common::POLL_INTERVAL, "cloud polling").await?;
    }
}

fn select_result<'a>(
    results: &'a [CloudExtractResult],
    expected_name: &str,
) -> Result<&'a CloudExtractResult> {
    if let Some(result) = results
        .iter()
        .find(|result| result.file_name.as_deref() == Some(expected_name))
    {
        return Ok(result);
    }
    match results {
        [result] => Ok(result),
        _ => Err(common::invalid_upstream(
            "the API returned no matching extract result",
        )),
    }
}

fn is_transient(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn validate_batch_id(batch_id: &str) -> Result<()> {
    if batch_id.is_empty()
        || batch_id.len() > 256
        || !batch_id
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
    {
        return Err(common::invalid_upstream(
            "the API returned an invalid batch ID",
        ));
    }
    Ok(())
}

fn endpoint(base: &Url, segments: &[&str], context: &str) -> Result<Url> {
    let mut url = base.clone();
    {
        let mut path = url
            .path_segments_mut()
            .map_err(|_| common::invalid_upstream(context))?;
        path.pop_if_empty();
        for segment in segments {
            path.push(segment);
        }
    }
    Ok(url)
}
