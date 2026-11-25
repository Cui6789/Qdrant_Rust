//! Small formatting and browser helpers.

/// Formats an integer with thousands separators.
pub fn format_int(value: u64) -> String {
    let digits = value.to_string();
    let mut output = String::with_capacity(digits.len() + digits.len() / 3);
    for (index, character) in digits.chars().enumerate() {
        if index > 0 && (digits.len() - index).is_multiple_of(3) {
            output.push(',');
        }
        output.push(character);
    }
    output
}

/// Formats a duration in milliseconds for status labels.
pub fn format_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms} ms")
    } else if ms < 60_000 {
        format!("{:.1} s", ms as f64 / 1000.0)
    } else {
        format!("{} min {:.0} s", ms / 60_000, (ms % 60_000) as f64 / 1000.0)
    }
}

/// Formats an RFC 3339 timestamp in the viewer's locale.
pub fn format_time(value: &str) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    if date.get_time().is_nan() {
        return value.to_string();
    }
    date.to_locale_string("zh-CN", &wasm_bindgen::JsValue::UNDEFINED)
        .as_string()
        .unwrap_or_else(|| value.to_string())
}

/// Formats an RFC 3339 timestamp as `HH:MM:SS` local time.
pub fn format_clock(value: &str) -> String {
    let date = js_sys::Date::new(&wasm_bindgen::JsValue::from_str(value));
    if date.get_time().is_nan() {
        return value.to_string();
    }
    format!(
        "{:02}:{:02}:{:02}",
        date.get_hours(),
        date.get_minutes(),
        date.get_seconds()
    )
}

/// Truncates text to `limit` characters, adding an ellipsis.
pub fn truncate(text: &str, limit: usize) -> String {
    let mut output = String::new();
    for (index, character) in text.chars().enumerate() {
        if index >= limit {
            output.push('…');
            break;
        }
        output.push(character);
    }
    output
}

/// Infers a document format from a file name, matching the ingestion rules.
pub fn infer_format(filename: &str) -> &'static str {
    let extension = filename
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "md" | "markdown" => "markdown",
        "html" | "htm" => "html",
        "json" => "json",
        _ => "text",
    }
}

/// Removes the final extension from a file name.
pub fn stem(filename: &str) -> String {
    filename
        .rsplit_once('.')
        .map(|(stem, _)| stem.to_string())
        .unwrap_or_else(|| filename.to_string())
}

/// Smoothly scrolls to an element id.
pub fn scroll_to(id: &str) {
    if let Some(window) = web_sys::window() {
        if let Some(document) = window.document() {
            if let Some(element) = document.get_element_by_id(id) {
                element.scroll_into_view();
            }
        }
    }
}

/// Label for the current embedding provider.
pub fn embedding_label(status: &kb_dashboard_types::EmbeddingStatus) -> String {
    match status.kind.as_deref() {
        Some("mock") => "本地演示嵌入".to_string(),
        Some("api") => status
            .model
            .clone()
            .unwrap_or_else(|| "API 嵌入".to_string()),
        _ => "未配置".to_string(),
    }
}

/// Shows only the host portion of an endpoint for compact status labels.
pub fn host_label(url: &str) -> String {
    let trimmed = url.trim();
    for prefix in ["https://", "http://"] {
        if let Some(rest) = trimmed.strip_prefix(prefix) {
            return rest.split('/').next().unwrap_or(rest).to_string();
        }
    }
    trimmed.to_string()
}
