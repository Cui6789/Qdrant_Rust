//! MinerU adapters for the legacy and current local APIs and cloud batch parsing.

mod cloud;
mod common;
mod file_parse;
mod local;

#[cfg(test)]
mod tests;

use std::fmt;
use std::time::Duration;

use reqwest::{redirect::Policy, Client, Url};
use serde::{Deserialize, Serialize};
use tokio::time::Instant;

use crate::error::{KbError, Result};

/// Default MinerU 3.x local endpoint.
pub const DEFAULT_LOCAL_URL: &str = "http://127.0.0.1:8000/file_parse";
/// Default MinerU cloud v4 API root.
pub const DEFAULT_CLOUD_URL: &str = "https://mineru.net/api/v4";
/// Default cloud model. `vlm` is MinerU's recommended precise-parse model.
pub const DEFAULT_MODEL_VERSION: &str = "vlm";
/// Default end-to-end budget for upload, polling, download, and extraction.
pub const DEFAULT_TIMEOUT_SECS: u64 = 600;

/// Selects the MinerU transport.
#[derive(Clone, Copy, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MineruMode {
    /// MinerU 3.x synchronous multipart `/file_parse`.
    Local,
    /// MinerU cloud v4 asynchronous batch API.
    Cloud,
}

/// MinerU adapter configuration.
#[derive(Clone, Deserialize, Serialize)]
#[serde(default)]
pub struct MineruConfig {
    /// Full local POST endpoint, normally ending in `/file_parse`.
    pub local_url: String,
    /// Cloud v4 API base, normally `https://mineru.net/api/v4`.
    pub cloud_url: String,
    /// Cloud API token. It may be empty while local mode is in use.
    pub api_key: String,
    /// Cloud model: `pipeline`, `vlm`, or `MinerU-HTML`.
    pub model_version: String,
    /// Total time budget for one parse operation.
    pub timeout_secs: u64,
}

impl Default for MineruConfig {
    fn default() -> Self {
        Self {
            local_url: DEFAULT_LOCAL_URL.to_owned(),
            cloud_url: DEFAULT_CLOUD_URL.to_owned(),
            api_key: String::new(),
            model_version: DEFAULT_MODEL_VERSION.to_owned(),
            timeout_secs: DEFAULT_TIMEOUT_SECS,
        }
    }
}

impl fmt::Debug for MineruConfig {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MineruConfig")
            .field("local_url", &self.local_url)
            .field("cloud_url", &self.cloud_url)
            .field("api_key", &"[redacted]")
            .field("model_version", &self.model_version)
            .field("timeout_secs", &self.timeout_secs)
            .finish()
    }
}

/// Validates both transport configurations.
///
/// An empty cloud token is valid here so applications can construct a parser
/// for local parsing. Cloud mode requires the token when `parse` is called.
pub fn validate_config(config: &MineruConfig) -> Result<()> {
    validate_endpoint(
        "mineru.local_url",
        &config.local_url,
        EndpointKind::FullEndpoint,
    )?;
    validate_endpoint("mineru.cloud_url", &config.cloud_url, EndpointKind::Base)?;

    if config.timeout_secs == 0 {
        return Err(KbError::Config(
            "mineru.timeout_secs must be greater than 0".to_owned(),
        ));
    }
    let timeout = Duration::from_secs(config.timeout_secs);
    if Instant::now().checked_add(timeout).is_none() {
        return Err(KbError::Config(
            "mineru.timeout_secs is too large".to_owned(),
        ));
    }

    let model = config.model_version.trim();
    if !matches!(model, "pipeline" | "vlm" | "MinerU-HTML") {
        return Err(KbError::Config(
            "mineru.model_version must be pipeline, vlm, or MinerU-HTML".to_owned(),
        ));
    }

    let api_key = config.api_key.trim();
    if !api_key.is_empty() && (!api_key.is_ascii() || api_key.chars().any(char::is_control)) {
        return Err(KbError::Config(
            "mineru.api_key contains invalid HTTP header characters".to_owned(),
        ));
    }
    Ok(())
}

/// Reusable MinerU client for both local and cloud parsing.
pub struct MineruParser {
    api_client: Client,
    transfer_client: Client,
    local_url: Url,
    cloud_base: Url,
    api_key: String,
    model_version: String,
    timeout: Duration,
}

impl MineruParser {
    /// Builds a parser and validates the configured endpoints.
    pub fn new(config: &MineruConfig) -> Result<Self> {
        validate_config(config)?;

        let local_url = parse_http_url("mineru.local_url", config.local_url.trim())?;
        let cloud_base = parse_http_url("mineru.cloud_url", config.cloud_url.trim())?;
        if cloud_base.query().is_some() || cloud_base.fragment().is_some() {
            return Err(KbError::Config(
                "mineru.cloud_url must not contain a query or fragment".to_owned(),
            ));
        }
        let cloud_base = base_with_trailing_slash(cloud_base)?;

        let timeout = Duration::from_secs(config.timeout_secs);
        let connect_timeout = Duration::from_secs(config.timeout_secs.min(10));
        let api_client = build_client(connect_timeout)?;
        let transfer_client = build_client(connect_timeout)?;

        Ok(Self {
            api_client,
            transfer_client,
            local_url,
            cloud_base,
            api_key: config.api_key.trim().to_owned(),
            model_version: config.model_version.trim().to_owned(),
            timeout,
        })
    }

    /// Parses one source document and returns its Markdown.
    ///
    /// The timeout is a single total budget shared by every upload, poll,
    /// download, and archive-extraction stage.
    pub async fn parse(&self, mode: MineruMode, name: &str, bytes: Vec<u8>) -> Result<String> {
        let deadline = Instant::now()
            .checked_add(self.timeout)
            .ok_or_else(|| KbError::Config("mineru.timeout_secs is too large".to_owned()))?;

        if bytes.is_empty() {
            return Err(KbError::Validation(
                "MinerU input document is empty".to_owned(),
            ));
        }
        if bytes.len() as u64 > common::MAX_INPUT_BYTES {
            return Err(KbError::PayloadTooLarge(format!(
                "MinerU input exceeds {} MiB",
                common::MAX_INPUT_BYTES / common::MIB
            )));
        }

        let name = sanitize_filename(name)?;
        let modern_local = mode == MineruMode::Local
            && self
                .local_url
                .path()
                .trim_end_matches('/')
                .ends_with("/v1/file_parse");
        if !modern_local {
            validate_input_format(mode, &name)?;
        }

        match mode {
            MineruMode::Local if modern_local => {
                file_parse::parse(self, &name, bytes, deadline).await
            }
            MineruMode::Local => local::parse(self, &name, bytes, deadline).await,
            MineruMode::Cloud => cloud::parse(self, &name, bytes, deadline).await,
        }
    }
}

impl fmt::Debug for MineruParser {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("MineruParser")
            .field("local_url", &self.local_url)
            .field("cloud_base", &self.cloud_base)
            .field("api_key", &"[redacted]")
            .field("model_version", &self.model_version)
            .field("timeout", &self.timeout)
            .finish()
    }
}

fn build_client(connect_timeout: Duration) -> Result<Client> {
    Client::builder()
        // API credentials and presigned URLs are never forwarded through a
        // redirect. A 3xx response is treated as an upstream failure.
        .redirect(Policy::none())
        .connect_timeout(connect_timeout)
        .build()
        .map_err(|_| {
            KbError::ParserUnavailable("MinerU could not initialize its HTTP client".to_owned())
        })
}

fn parse_http_url(field: &str, value: &str) -> Result<Url> {
    let url = Url::parse(value)
        .map_err(|_| KbError::Config(format!("{field} must be an absolute URL")))?;
    if !matches!(url.scheme(), "http" | "https") || url.host_str().is_none() {
        return Err(KbError::Config(format!("{field} must use http or https")));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(KbError::Config(format!(
            "{field} must not contain embedded credentials"
        )));
    }
    Ok(url)
}

enum EndpointKind {
    FullEndpoint,
    Base,
}

fn validate_endpoint(field: &str, value: &str, kind: EndpointKind) -> Result<()> {
    let value = value.trim();
    if value.is_empty() {
        return Err(KbError::Config(format!("{field} must not be empty")));
    }
    let url = parse_http_url(field, value)?;
    if url.query().is_some() || url.fragment().is_some() {
        return Err(KbError::Config(format!(
            "{field} must not contain a query or fragment"
        )));
    }
    if matches!(kind, EndpointKind::FullEndpoint) && url.path() == "/" {
        return Err(KbError::Config(format!(
            "{field} must be the full parse endpoint"
        )));
    }
    Ok(())
}

fn base_with_trailing_slash(mut url: Url) -> Result<Url> {
    if !url.path().ends_with('/') {
        let path = format!("{}/", url.path().trim_end_matches('/'));
        url.set_path(&path);
    }
    if url.cannot_be_a_base() {
        return Err(KbError::Config(
            "mineru.cloud_url must be a hierarchical URL".to_owned(),
        ));
    }
    Ok(url)
}

fn sanitize_filename(name: &str) -> Result<String> {
    let basename = name.rsplit(['/', '\\']).next().unwrap_or_default().trim();
    if matches!(basename, "" | "." | "..") {
        return Err(KbError::Parse(
            "MinerU input filename is invalid".to_owned(),
        ));
    }

    let mut cleaned = String::with_capacity(basename.len());
    for character in basename.chars() {
        if character.is_control() || matches!(character, ':' | '*' | '?' | '"' | '<' | '>' | '|') {
            cleaned.push('_');
        } else {
            cleaned.push(character);
        }
    }

    let cleaned = cleaned.trim();
    if matches!(cleaned, "" | "." | "..") {
        return Err(KbError::Parse(
            "MinerU input filename is invalid".to_owned(),
        ));
    }
    Ok(truncate_filename(cleaned, common::MAX_FILENAME_BYTES))
}

fn truncate_filename(name: &str, max_bytes: usize) -> String {
    if name.len() <= max_bytes {
        return name.to_owned();
    }

    let extension = name
        .rfind('.')
        .filter(|index| *index > 0 && name.len() - index <= 20)
        .map(|index| &name[index..]);
    let stem = extension
        .and_then(|extension| name.strip_suffix(extension))
        .unwrap_or(name);
    let extension_len = extension.map_or(0, str::len);
    let stem_limit = max_bytes.saturating_sub(extension_len);

    let mut truncated = String::new();
    for character in stem.chars() {
        if truncated.len() + character.len_utf8() > stem_limit {
            break;
        }
        truncated.push(character);
    }
    if let Some(extension) = extension {
        truncated.push_str(extension);
    }
    truncated
}

fn validate_input_format(mode: MineruMode, name: &str) -> Result<()> {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .ok_or_else(|| {
            KbError::UnsupportedFormat(
                "MinerU input filename must include a file extension".to_owned(),
            )
        })?;

    let supported = match mode {
        MineruMode::Local => matches!(
            extension.as_str(),
            "pdf"
                | "docx"
                | "pptx"
                | "xlsx"
                | "png"
                | "jpg"
                | "jpeg"
                | "jp2"
                | "webp"
                | "gif"
                | "bmp"
                | "tiff"
        ),
        MineruMode::Cloud => matches!(
            extension.as_str(),
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
                | "jp2"
                | "webp"
                | "gif"
                | "bmp"
                | "html"
        ),
    };
    if !supported {
        return Err(KbError::UnsupportedFormat(format!(
            "MinerU {} mode does not accept .{extension} files",
            match mode {
                MineruMode::Local => "local",
                MineruMode::Cloud => "cloud",
            }
        )));
    }
    Ok(())
}

pub(super) fn cloud_model_for_input<'a>(configured: &'a str, name: &str) -> Result<&'a str> {
    let is_html = name
        .rsplit_once('.')
        .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("html"));
    match (is_html, configured) {
        (true, "MinerU-HTML") | (false, "pipeline" | "vlm") => Ok(configured),
        (true, _) => Err(KbError::Validation(
            "MinerU cloud HTML input requires model_version = MinerU-HTML".to_owned(),
        )),
        (false, _) => Err(KbError::Validation(
            "MinerU-HTML model_version is only valid for HTML input".to_owned(),
        )),
    }
}
