use std::io::{Cursor, Write};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

use serde_json::json;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, Request, ResponseTemplate};

use super::{validate_config, MineruConfig, MineruMode, MineruParser};
use crate::error::KbError;

#[test]
fn config_defaults_and_mode_names_match_the_public_contract() {
    let config: MineruConfig = serde_json::from_value(json!({})).unwrap();
    assert_eq!(config.local_url, "http://127.0.0.1:8000/file_parse");
    assert_eq!(config.cloud_url, "https://mineru.net/api/v4");
    assert_eq!(config.model_version, "vlm");
    assert_eq!(config.timeout_secs, 600);
    assert!(config.api_key.is_empty());
    assert_eq!(serde_json::to_value(MineruMode::Local).unwrap(), "local");
    assert_eq!(serde_json::to_value(MineruMode::Cloud).unwrap(), "cloud");
    validate_config(&config).unwrap();
    MineruParser::new(&config).unwrap();
}

#[tokio::test]
async fn local_mode_uses_the_3x_multipart_contract_and_sanitizes_the_name() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/file_parse"))
        .respond_with(|request: &Request| {
            assert!(request.headers.get("authorization").is_none());
            let content_type = request
                .headers
                .get("content-type")
                .unwrap()
                .to_str()
                .unwrap();
            assert!(content_type.starts_with("multipart/form-data; boundary="));
            let body = std::str::from_utf8(&request.body).unwrap();
            assert!(body.contains("name=\"files\"; filename=\"notes.pdf\""));
            assert!(body.contains("name=\"parse_method\"\r\n\r\nauto"));
            assert!(body.contains("name=\"return_md\"\r\n\r\ntrue"));
            assert!(body.contains("name=\"response_format_zip\"\r\n\r\nfalse"));
            ResponseTemplate::new(200).set_body_json(json!({
                "backend": "pipeline",
                "version": "3.4.5",
                "results": {
                    "notes": {
                        "md_content": "# Local MinerU\n\nBody"
                    }
                }
            }))
        })
        .expect(1)
        .mount(&server)
        .await;

    let config = MineruConfig {
        local_url: format!("{}/file_parse", server.uri()),
        ..MineruConfig::default()
    };
    let parser = MineruParser::new(&config).unwrap();
    let markdown = parser
        .parse(
            MineruMode::Local,
            "../../unsafe\\notes.pdf",
            b"%PDF-local".to_vec(),
        )
        .await
        .unwrap();

    assert_eq!(markdown, "# Local MinerU\n\nBody");
}

#[tokio::test]
async fn cloud_mode_signs_uploads_without_api_auth_and_reads_full_md_in_memory() {
    let server = MockServer::start().await;
    let upload_url = format!("{}/upload?signature=upload-secret", server.uri());
    let zip_url = format!("{}/result.zip?signature=download-secret", server.uri());
    let upload_requests = Arc::new(AtomicUsize::new(0));
    let download_requests = Arc::new(AtomicUsize::new(0));

    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .and(header("Authorization", "Bearer sk-cloud-secret"))
        .respond_with({
            let upload_url = upload_url.clone();
            move |request: &Request| {
                let body: serde_json::Value = serde_json::from_slice(&request.body).unwrap();
                assert_eq!(body["files"][0]["name"], "notes.pdf");
                assert_eq!(body["model_version"], "vlm");
                ResponseTemplate::new(200).set_body_json(json!({
                    "code": 0,
                    "msg": "ok",
                    "data": {
                        "batch_id": "batch-123",
                        "file_urls": [upload_url]
                    }
                }))
            }
        })
        .expect(1)
        .mount(&server)
        .await;

    Mock::given(method("PUT"))
        .and(path("/upload"))
        .respond_with({
            let upload_requests = upload_requests.clone();
            move |request: &Request| {
                upload_requests.fetch_add(1, Ordering::SeqCst);
                assert!(request.headers.get("authorization").is_none());
                assert!(request.headers.get("content-type").is_none());
                assert_eq!(request.body, b"%PDF-cloud");
                ResponseTemplate::new(200)
            }
        })
        .expect(1)
        .mount(&server)
        .await;

    let polls = Arc::new(AtomicUsize::new(0));
    Mock::given(method("GET"))
        .and(path("/api/v4/extract-results/batch/batch-123"))
        .and(header("Authorization", "Bearer sk-cloud-secret"))
        .respond_with({
            let zip_url = zip_url.clone();
            let polls = polls.clone();
            move |_request: &Request| {
                let poll = polls.fetch_add(1, Ordering::SeqCst);
                let state = if poll == 0 { "running" } else { "done" };
                let mut result = json!({
                    "file_name": "notes.pdf",
                    "state": state,
                    "err_msg": "",
                    "data_id": "item-1"
                });
                if state == "done" {
                    result["full_zip_url"] = json!(zip_url);
                    result["extract_progress"] = json!({
                        "extracted_pages": 2,
                        "total_pages": 2,
                        "start_time": "2026-11-27 10:00:00"
                    });
                } else {
                    result["extract_progress"] = json!({
                        "extracted_pages": 1,
                        "total_pages": 2,
                        "start_time": "2026-11-27 10:00:00"
                    });
                }
                ResponseTemplate::new(200).set_body_json(json!({
                    "code": 0,
                    "msg": "ok",
                    "trace_id": "trace-1",
                    "data": {
                        "batch_id": "batch-123",
                        "extract_result": [result]
                    }
                }))
            }
        })
        .expect(2)
        .mount(&server)
        .await;

    let archive = zip_with_full_md("# Cloud MinerU\n\nArchive body");
    Mock::given(method("GET"))
        .and(path("/result.zip"))
        .respond_with({
            let archive = archive.clone();
            let download_requests = download_requests.clone();
            move |request: &Request| {
                download_requests.fetch_add(1, Ordering::SeqCst);
                assert!(request.headers.get("authorization").is_none());
                ResponseTemplate::new(200).set_body_raw(archive.clone(), "application/zip")
            }
        })
        .expect(1)
        .mount(&server)
        .await;

    let config = MineruConfig {
        cloud_url: format!("{}/api/v4", server.uri()),
        api_key: "sk-cloud-secret".to_owned(),
        timeout_secs: 5,
        ..MineruConfig::default()
    };
    let parser = MineruParser::new(&config).unwrap();
    let markdown = parser
        .parse(MineruMode::Cloud, "notes.pdf", b"%PDF-cloud".to_vec())
        .await
        .unwrap();

    assert_eq!(markdown, "# Cloud MinerU\n\nArchive body");
    assert_eq!(upload_requests.load(Ordering::SeqCst), 1);
    assert_eq!(download_requests.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn upstream_failure_does_not_echo_the_key_or_signed_url() {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/api/v4/file-urls/batch"))
        .respond_with(ResponseTemplate::new(401).set_body_string(
            "sk-cloud-secret https://storage.invalid/upload?signature=signed-secret",
        ))
        .expect(1)
        .mount(&server)
        .await;

    let config = MineruConfig {
        cloud_url: format!("{}/api/v4", server.uri()),
        api_key: "sk-cloud-secret".to_owned(),
        ..MineruConfig::default()
    };
    let parser = MineruParser::new(&config).unwrap();
    let error = parser
        .parse(MineruMode::Cloud, "notes.pdf", b"%PDF".to_vec())
        .await
        .unwrap_err();
    let rendered = error.to_string();

    assert!(matches!(error, KbError::ParserUnavailable(_)));
    assert!(!rendered.contains("sk-cloud-secret"));
    assert!(!rendered.contains("signed-secret"));
    assert!(!rendered.contains("storage.invalid"));
}

#[test]
fn result_archive_requires_exactly_one_full_md() {
    let missing = zip_with_entries(&[("document.md", b"# Wrong name")]);
    let error =
        super::common::markdown_from_zip(&missing, "cloud result", Some("full.md")).unwrap_err();
    assert!(matches!(error, KbError::ParserUnavailable(_)));

    let duplicate = zip_with_entries(&[("one/full.md", b"# One"), ("two/full.md", b"# Two")]);
    let error =
        super::common::markdown_from_zip(&duplicate, "cloud result", Some("full.md")).unwrap_err();
    assert!(matches!(error, KbError::ParserUnavailable(_)));
}

fn zip_with_full_md(markdown: &str) -> Vec<u8> {
    zip_with_entries(&[
        ("images/ignored.png", b"not-an-image"),
        ("full.md", markdown.as_bytes()),
    ])
}

fn zip_with_entries(entries: &[(&str, &[u8])]) -> Vec<u8> {
    let mut writer = zip::ZipWriter::new(Cursor::new(Vec::new()));
    for (name, contents) in entries {
        writer
            .start_file(*name, zip::write::SimpleFileOptions::default())
            .unwrap();
        writer.write_all(contents).unwrap();
    }
    writer.finish().unwrap().into_inner()
}
