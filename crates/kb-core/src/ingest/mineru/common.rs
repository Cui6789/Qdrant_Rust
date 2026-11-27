use std::io::{Cursor, Read};
use std::time::Duration;

use reqwest::{Response, StatusCode, Url};
use serde::de::DeserializeOwned;
use tokio::time::Instant;

use crate::error::{KbError, Result};

pub(super) const MIB: u64 = 1024 * 1024;
pub(super) const MAX_INPUT_BYTES: u64 = 200 * MIB;
pub(super) const MAX_API_JSON_BYTES: u64 = MIB;
pub(super) const MAX_LOCAL_JSON_BYTES: u64 = 64 * MIB;
pub(super) const MAX_ZIP_BYTES: u64 = 512 * MIB;
pub(super) const MAX_MARKDOWN_BYTES: u64 = 32 * MIB;
pub(super) const MAX_ARCHIVE_ENTRIES: usize = 20_000;
pub(super) const MAX_FILENAME_BYTES: usize = 255;

#[cfg(not(test))]
pub(super) const POLL_INTERVAL: Duration = Duration::from_secs(2);
#[cfg(test)]
pub(super) const POLL_INTERVAL: Duration = Duration::from_millis(10);

const MAX_POLL_INTERVAL: Duration = Duration::from_secs(30);

pub(super) fn remaining(deadline: Instant, stage: &str) -> Result<Duration> {
    let now = Instant::now();
    let remaining = deadline.saturating_duration_since(now);
    if remaining.is_zero() {
        return Err(deadline_error(stage));
    }
    Ok(remaining)
}

pub(super) fn deadline_error(stage: &str) -> KbError {
    KbError::ParserUnavailable(format!("MinerU {stage} exceeded the total deadline"))
}

pub(super) fn request_error(stage: &str, error: &reqwest::Error) -> KbError {
    if error.is_timeout() {
        deadline_error(stage)
    } else {
        KbError::ParserUnavailable(format!("MinerU {stage} request failed"))
    }
}

pub(super) fn invalid_upstream(message: &str) -> KbError {
    KbError::ParserUnavailable(format!("MinerU cloud: {message}"))
}

pub(super) fn status_error(service: &str, status: StatusCode) -> KbError {
    let message = format!("MinerU {service} returned HTTP {status}");
    match status {
        StatusCode::BAD_REQUEST => KbError::Parse(message),
        StatusCode::PAYLOAD_TOO_LARGE => KbError::PayloadTooLarge(message),
        StatusCode::UNSUPPORTED_MEDIA_TYPE => KbError::UnsupportedFormat(message),
        _ => KbError::ParserUnavailable(message),
    }
}

pub(super) async fn read_body_limited(
    response: &mut Response,
    limit: u64,
    stage: &str,
    deadline: Instant,
) -> Result<Vec<u8>> {
    if response
        .content_length()
        .is_some_and(|length| length > limit)
    {
        return Err(payload_too_large(stage, limit));
    }

    let capacity = response
        .content_length()
        .unwrap_or(0)
        .min(limit)
        .min(usize::MAX as u64) as usize;
    let mut body = Vec::with_capacity(capacity);
    while body.len() as u64 <= limit {
        let chunk = tokio::time::timeout_at(deadline, response.chunk())
            .await
            .map_err(|_| deadline_error(stage))?
            .map_err(|error| request_error(stage, &error))?;
        let Some(chunk) = chunk else {
            return Ok(body);
        };
        if chunk.len() as u64 > limit.saturating_sub(body.len() as u64) {
            return Err(payload_too_large(stage, limit));
        }
        body.extend_from_slice(&chunk);
    }
    Err(payload_too_large(stage, limit))
}

pub(super) async fn read_json_limited<T: DeserializeOwned>(
    mut response: Response,
    limit: u64,
    stage: &str,
    deadline: Instant,
) -> Result<T> {
    let body = read_body_limited(&mut response, limit, stage, deadline).await?;
    serde_json::from_slice(&body)
        .map_err(|_| KbError::ParserUnavailable(format!("MinerU {stage} returned invalid JSON")))
}

pub(super) async fn sleep_until(deadline: Instant, delay: Duration, stage: &str) -> Result<()> {
    let remaining = remaining(deadline, stage)?;
    if delay >= remaining {
        tokio::time::sleep(remaining).await;
        return Err(deadline_error(stage));
    }
    tokio::time::sleep(delay).await;
    Ok(())
}

pub(super) fn retry_after(response: &Response) -> Option<Duration> {
    let value = response
        .headers()
        .get(reqwest::header::RETRY_AFTER)?
        .to_str()
        .ok()?
        .trim()
        .parse::<u64>()
        .ok()?;
    Some(
        Duration::from_secs(value)
            .max(POLL_INTERVAL)
            .min(MAX_POLL_INTERVAL),
    )
}

pub(super) fn validate_transfer_url(value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|_| invalid_upstream("the API returned an invalid transfer URL"))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(invalid_upstream("the API returned a non-HTTP transfer URL"));
    }
    if url.fragment().is_some() || !url.username().is_empty() || url.password().is_some() {
        return Err(invalid_upstream(
            "the API returned a transfer URL with credentials or a fragment",
        ));
    }
    Ok(url)
}

pub(super) async fn read_markdown_zip(
    mut response: Response,
    stage: &'static str,
    filename: Option<&'static str>,
    deadline: Instant,
) -> Result<String> {
    let bytes = read_body_limited(&mut response, MAX_ZIP_BYTES, stage, deadline).await?;
    let task = tokio::task::spawn_blocking(move || markdown_from_zip(&bytes, stage, filename));
    tokio::time::timeout_at(deadline, task)
        .await
        .map_err(|_| deadline_error(stage))?
        .map_err(|_| KbError::ParserUnavailable(format!("MinerU could not unpack {stage}")))?
}

pub(super) fn markdown_from_zip(
    bytes: &[u8],
    stage: &str,
    filename: Option<&str>,
) -> Result<String> {
    let invalid = |message: &str| KbError::ParserUnavailable(format!("MinerU {stage}: {message}"));
    let mut archive =
        zip::ZipArchive::new(Cursor::new(bytes)).map_err(|_| invalid("invalid ZIP"))?;
    if archive.len() > MAX_ARCHIVE_ENTRIES {
        return Err(KbError::PayloadTooLarge(
            "MinerU ZIP contains too many entries".into(),
        ));
    }

    let expected = filename.unwrap_or("Markdown file");
    let mut selected = None;
    for index in 0..archive.len() {
        let entry = archive
            .by_index_raw(index)
            .map_err(|_| invalid("unreadable ZIP entry"))?;
        let basename = entry.name().rsplit(['/', '\\']).next().unwrap_or_default();
        let matches = filename.map_or_else(
            || {
                basename
                    .rsplit_once('.')
                    .is_some_and(|(_, ext)| ext.eq_ignore_ascii_case("md"))
            },
            |name| basename == name,
        );
        if entry.is_file() && matches && selected.replace(index).is_some() {
            return Err(invalid(&format!(
                "the result ZIP contains more than one {expected}"
            )));
        }
    }

    let index =
        selected.ok_or_else(|| invalid(&format!("the result ZIP contains no {expected}")))?;
    let entry = archive
        .by_index(index)
        .map_err(|_| invalid("unreadable Markdown in ZIP"))?;
    if entry.size() > MAX_MARKDOWN_BYTES {
        return Err(payload_too_large("Markdown", MAX_MARKDOWN_BYTES));
    }
    let mut content = Vec::with_capacity(entry.size().min(MIB * 4) as usize);
    entry
        .take(MAX_MARKDOWN_BYTES + 1)
        .read_to_end(&mut content)
        .map_err(|_| invalid("unreadable Markdown in ZIP"))?;
    if content.len() as u64 > MAX_MARKDOWN_BYTES {
        return Err(payload_too_large("Markdown", MAX_MARKDOWN_BYTES));
    }
    let markdown =
        String::from_utf8(content).map_err(|_| invalid("Markdown is not valid UTF-8"))?;
    let markdown = markdown.trim_start_matches('\u{feff}').trim();
    if markdown.is_empty() {
        return Err(invalid("Markdown is empty"));
    }
    Ok(markdown.to_owned())
}

fn payload_too_large(stage: &str, limit: u64) -> KbError {
    KbError::PayloadTooLarge(format!(
        "MinerU {stage} exceeds the {} MiB limit",
        limit / MIB
    ))
}
