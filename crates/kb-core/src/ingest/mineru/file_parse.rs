//! Adapter for the MinerU 4.x file-parse wrapper.

use reqwest::{multipart, RequestBuilder, Response, StatusCode, Url};
use serde::Deserialize;
use tokio::time::Instant;

use crate::error::{KbError, Result};

use super::{common, MineruParser};

const STAGE: &str = "local parsing";

pub(super) async fn parse(
    parser: &MineruParser,
    filename: &str,
    bytes: Vec<u8>,
    deadline: Instant,
) -> Result<String> {
    let id = uuid::Uuid::new_v4().simple().to_string();
    let form = multipart::Form::new()
        .part(
            "file",
            multipart::Part::bytes(bytes).file_name(filename.to_owned()),
        )
        .text("output_formats", "default")
        .text("wait_timeout", "0")
        .text("request_id", id.clone());
    let response = send(
        parser
            .api_client
            .post(parser.local_url.clone())
            .multipart(form),
        deadline,
    )
    .await?;
    match response.status() {
        StatusCode::OK => return attachment(response, deadline).await,
        StatusCode::ACCEPTED => {}
        status => return Err(common::status_error("local parser", status)),
    }

    // Use the configured service origin for both recovery requests.
    let status_url = subpath(&parser.local_url, &id)?;
    let result_url = subpath(&status_url, "result")?;
    let mut current = task_status(response, &id, deadline).await?;

    loop {
        match current.status.as_str() {
            "completed" => {
                let response = send(parser.api_client.get(result_url.clone()), deadline).await?;
                match response.status() {
                    StatusCode::OK => return attachment(response, deadline).await,
                    StatusCode::CONFLICT => {}
                    status => return Err(common::status_error("local parser", status)),
                }
            }
            "prepared" | "submitting" | "waiting-file" | "pending" | "running" => {}
            "failed" | "canceled" => {
                return Err(unavailable("the parse task failed or was canceled"))
            }
            "submission_unknown" => {
                return Err(unavailable("the upstream task could not be confirmed"))
            }
            _ => return Err(unavailable("unknown parse task status")),
        }

        common::sleep_until(deadline, common::POLL_INTERVAL, STAGE).await?;
        let response = send(parser.api_client.get(status_url.clone()), deadline).await?;
        if response.status() != StatusCode::OK {
            return Err(common::status_error("local parser", response.status()));
        }
        current = task_status(response, &id, deadline).await?;
    }
}

fn subpath(endpoint: &Url, segment: &str) -> Result<Url> {
    let mut endpoint = endpoint.clone();
    endpoint
        .path_segments_mut()
        .map_err(|_| unavailable("invalid local endpoint"))?
        .pop_if_empty()
        .push(segment);
    Ok(endpoint)
}

async fn send(request: RequestBuilder, deadline: Instant) -> Result<Response> {
    tokio::time::timeout_at(deadline, request.send())
        .await
        .map_err(|_| common::deadline_error(STAGE))?
        .map_err(|error| common::request_error(STAGE, &error))
}

#[derive(Deserialize)]
struct TaskStatus {
    request_id: String,
    status: String,
}

async fn task_status(response: Response, id: &str, deadline: Instant) -> Result<TaskStatus> {
    let status: TaskStatus =
        common::read_json_limited(response, common::MAX_API_JSON_BYTES, STAGE, deadline).await?;
    if status.request_id != id {
        return Err(unavailable("unexpected parse request ID"));
    }
    Ok(status)
}

async fn attachment(response: Response, deadline: Instant) -> Result<String> {
    common::read_markdown_zip(response, "local result", None, deadline).await
}

fn unavailable(message: &str) -> KbError {
    KbError::ParserUnavailable(format!("MinerU local: {message}"))
}
#[cfg(test)]
mod tests {
    use std::io::{Cursor, Write};
    use std::sync::{Arc, Mutex};

    use serde_json::json;
    use wiremock::matchers::{method, path, path_regex};
    use wiremock::{Mock, MockServer, Request, ResponseTemplate};

    use super::*;

    #[tokio::test]
    async fn polls_the_local_wrapper_and_extracts_markdown() {
        let server = MockServer::start().await;
        let identity = Arc::new(Mutex::new(String::new()));
        let id = identity.clone();
        Mock::given(method("POST"))
            .and(path("/v1/file_parse"))
            .respond_with(move |request: &Request| {
                let body = std::str::from_utf8(&request.body).unwrap();
                assert!(body.contains("name=\"file\"; filename=\"report.pdf\""));
                assert!(body.contains("name=\"wait_timeout\"\r\n\r\n0"));
                let generated = body
                    .split("name=\"request_id\"\r\n\r\n")
                    .nth(1)
                    .unwrap()
                    .split("\r\n")
                    .next()
                    .unwrap()
                    .to_owned();
                *id.lock().unwrap() = generated.clone();
                ResponseTemplate::new(202).set_body_json(json!({
                    "request_id": generated,
                    "status": "running"
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        Mock::given(method("GET"))
            .and(path_regex("^/v1/file_parse/[^/]+$"))
            .respond_with(move |request: &Request| {
                let id = identity.lock().unwrap().clone();
                assert_eq!(request.url.path(), format!("/v1/file_parse/{id}"));
                ResponseTemplate::new(200).set_body_json(json!({
                    "request_id": id,
                    "status": "completed"
                }))
            })
            .expect(1)
            .mount(&server)
            .await;

        let mut archive = zip::ZipWriter::new(Cursor::new(Vec::new()));
        archive
            .start_file("document.md", zip::write::SimpleFileOptions::default())
            .unwrap();
        archive
            .write_all(b"# Parsed PDF\n\nSearchable text.")
            .unwrap();
        let zip = archive.finish().unwrap().into_inner();
        Mock::given(method("GET"))
            .and(path_regex("^/v1/file_parse/[^/]+/result$"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(zip, "application/zip"))
            .expect(1)
            .mount(&server)
            .await;

        let parser = MineruParser::new(&super::super::MineruConfig {
            local_url: format!("{}/v1/file_parse", server.uri()),
            timeout_secs: 10,
            ..Default::default()
        })
        .unwrap();
        let content = parser
            .parse(
                super::super::MineruMode::Local,
                "C:\\upload\\report.pdf",
                b"%PDF".to_vec(),
            )
            .await
            .unwrap();
        assert_eq!(content, "# Parsed PDF\n\nSearchable text.");
    }
}
