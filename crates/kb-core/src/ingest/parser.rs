//! Document parsers and text decoding.
//!
//! The parser is the only place where raw bytes become normalized text.
//! Downstream chunking expects plain text with Markdown-style `#` headings,
//! which is why HTML headings are rewritten into heading lines.

use scraper::{Html, Node};
use serde_json::Value;

use crate::error::{KbError, Result};
use crate::models::DocumentFormat;

/// Leading UTF-8 byte-order mark, which is stripped before decoding.
const UTF8_BOM: &[u8] = &[0xEF, 0xBB, 0xBF];

/// Decodes document bytes into text.
///
/// `None` and `utf-8` use strict UTF-8 decoding (stripping a leading BOM);
/// `gbk` and `gb18030` are decoded through `encoding_rs`. Unknown labels and
/// byte sequences that are invalid in the selected encoding are rejected with
/// [`KbError::InvalidEncoding`].
pub fn decode_bytes(bytes: &[u8], encoding: Option<&str>) -> Result<String> {
    let label = encoding.unwrap_or("utf-8").trim().to_ascii_lowercase();
    match label.as_str() {
        "utf-8" | "utf8" => {
            let bytes = bytes.strip_prefix(UTF8_BOM).unwrap_or(bytes);
            let text = std::str::from_utf8(bytes).map_err(|error| {
                KbError::InvalidEncoding(format!("input is not valid UTF-8: {error}"))
            })?;
            Ok(text.to_owned())
        }
        "gbk" => decode_with(encoding_rs::GBK, bytes, &label),
        "gb18030" => decode_with(encoding_rs::GB18030, bytes, &label),
        other => Err(KbError::InvalidEncoding(format!(
            "unsupported encoding: '{other}'"
        ))),
    }
}

fn decode_with(
    encoding: &'static encoding_rs::Encoding,
    bytes: &[u8],
    label: &str,
) -> Result<String> {
    let (text, _, had_errors) = encoding.decode(bytes);
    if had_errors {
        Err(KbError::InvalidEncoding(format!(
            "input is not valid {label} text"
        )))
    } else {
        Ok(text.into_owned())
    }
}

/// Parses a decoded document into normalized plain text.
///
/// Line endings are normalized to `LF`, surrounding whitespace is trimmed and
/// HTML is reduced to visible text with Markdown-style headings. Documents that
/// contain no text (after parsing) are rejected with [`KbError::Validation`].
pub fn parse_text(text: &str, format: DocumentFormat) -> Result<String> {
    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let parsed = match format {
        DocumentFormat::Text | DocumentFormat::Markdown => normalized.trim().to_owned(),
        DocumentFormat::Html => html_to_text(&normalized),
        DocumentFormat::Json => json_to_text(&normalized)?,
    };
    if parsed.trim().is_empty() {
        return Err(KbError::Validation("document contains no text".to_owned()));
    }
    Ok(parsed.trim().to_owned())
}

/// Work items for the non-recursive HTML walk. `Close*` items are pushed before
/// a node's children so the boundary break is emitted after the whole subtree.
enum HtmlTask<N> {
    Node(N),
    CloseHeading,
    CloseBlock,
}

/// Converts HTML into line-oriented text, rewriting headings and list items.
/// Head metadata (`head`/`title`) is not visible body text and is skipped.
fn html_to_text(html: &str) -> String {
    let document = Html::parse_document(html);
    let mut output = String::new();
    let mut stack: Vec<HtmlTask<_>> = vec![HtmlTask::Node(document.tree.root())];

    while let Some(task) = stack.pop() {
        match task {
            HtmlTask::Node(node) => match node.value() {
                Node::Element(element) => {
                    let tag = element.name();
                    if let Some(level) = heading_level(tag) {
                        push_block_break(&mut output);
                        output.push_str(&"#".repeat(level));
                        output.push(' ');
                        stack.push(HtmlTask::CloseHeading);
                    } else {
                        match tag {
                            "script" | "style" | "noscript" | "head" | "title" => continue,
                            "li" => {
                                push_line_break(&mut output);
                                output.push_str("- ");
                            }
                            "br" => {
                                push_line_break(&mut output);
                                continue;
                            }
                            "p" | "div" | "section" | "article" | "header" | "footer" | "ul"
                            | "ol" | "table" | "tr" => {
                                push_block_break(&mut output);
                                stack.push(HtmlTask::CloseBlock);
                            }
                            _ => {}
                        }
                    }
                    let children: Vec<_> = node.children().collect();
                    stack.extend(children.into_iter().rev().map(HtmlTask::Node));
                }
                Node::Text(text) => push_text(&mut output, &text.text),
                _ => {
                    let children: Vec<_> = node.children().collect();
                    stack.extend(children.into_iter().rev().map(HtmlTask::Node));
                }
            },
            HtmlTask::CloseHeading | HtmlTask::CloseBlock => push_line_break(&mut output),
        }
    }

    normalize_html_output(&output)
}

fn heading_level(tag: &str) -> Option<usize> {
    match tag {
        "h1" => Some(1),
        "h2" => Some(2),
        "h3" => Some(3),
        "h4" => Some(4),
        "h5" => Some(5),
        "h6" => Some(6),
        _ => None,
    }
}

/// Appends text with internal whitespace runs collapsed to single spaces.
fn push_text(output: &mut String, raw: &str) {
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    if collapsed.is_empty() {
        return;
    }
    if starts_with_whitespace(raw) && !ends_with_whitespace(output) {
        output.push(' ');
    }
    output.push_str(&collapsed);
    if ends_with_whitespace(raw) {
        output.push(' ');
    }
}

fn starts_with_whitespace(value: &str) -> bool {
    value.chars().next().is_some_and(char::is_whitespace)
}

fn ends_with_whitespace(value: &str) -> bool {
    value.chars().last().is_some_and(char::is_whitespace)
}

/// Starts a new paragraph-like block; repeated breaks are collapsed later.
fn push_block_break(output: &mut String) {
    output.push_str("\n\n");
}

fn push_line_break(output: &mut String) {
    output.push('\n');
}

/// Collapses runs of 3+ newlines to a single blank line, trims line ends and
/// surrounding whitespace.
fn normalize_html_output(raw: &str) -> String {
    let mut collapsed = String::with_capacity(raw.len());
    let mut newline_run = 0;
    for character in raw.chars() {
        if character == '\n' {
            newline_run += 1;
            if newline_run <= 2 {
                collapsed.push('\n');
            }
        } else {
            newline_run = 0;
            collapsed.push(character);
        }
    }
    collapsed
        .lines()
        .map(str::trim_end)
        .collect::<Vec<_>>()
        .join("\n")
        .trim()
        .to_owned()
}

/// Converts JSON into line-oriented text.
///
/// Arrays of objects carrying a string `text` field and arrays of strings are
/// joined in order; any other document contributes every string leaf value.
fn json_to_text(json: &str) -> Result<String> {
    let value: Value = serde_json::from_str(json)
        .map_err(|error| KbError::Parse(format!("invalid JSON document: {error}")))?;

    if let Value::Array(items) = &value {
        if !items.is_empty()
            && items
                .iter()
                .all(|item| item.get("text").is_some_and(Value::is_string))
        {
            let lines = items
                .iter()
                .filter_map(|item| item.get("text").and_then(Value::as_str))
                .collect::<Vec<_>>();
            return Ok(lines.join("\n"));
        }
        if !items.is_empty() && items.iter().all(Value::is_string) {
            let lines = items.iter().filter_map(Value::as_str).collect::<Vec<_>>();
            return Ok(lines.join("\n"));
        }
    }

    let mut leaves = Vec::new();
    collect_string_leaves(&value, &mut leaves);
    Ok(leaves.join("\n"))
}

fn collect_string_leaves(value: &Value, leaves: &mut Vec<String>) {
    match value {
        Value::String(text) => leaves.push(text.clone()),
        Value::Array(items) => {
            for item in items {
                collect_string_leaves(item, leaves);
            }
        }
        Value::Object(map) => {
            for item in map.values() {
                collect_string_leaves(item, leaves);
            }
        }
        Value::Null | Value::Bool(_) | Value::Number(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalizes_text_and_markdown() {
        let markdown = "  # 标题\r\n正文\r\n";
        let parsed = parse_text(markdown, DocumentFormat::Markdown).expect("markdown parses");
        assert_eq!(parsed, "# 标题\n正文");
        assert!(!parsed.contains('\r'));

        let text = "  hello\r\nworld  ";
        assert_eq!(
            parse_text(text, DocumentFormat::Text).expect("text parses"),
            "hello\nworld"
        );
    }

    #[test]
    fn html_extracts_visible_text_and_headings() {
        let html = "<html><head><style>p{}</style></head><body><h2>标题</h2><p>正文</p><script>bad()</script><ul><li>条目</li></ul></body></html>";
        let parsed = parse_text(html, DocumentFormat::Html).expect("html parses");
        assert!(parsed.contains("## 标题"), "missing heading: {parsed:?}");
        assert!(parsed.contains("正文"), "missing paragraph: {parsed:?}");
        assert!(parsed.contains("条目"), "missing list item: {parsed:?}");
        assert!(!parsed.contains("p{}"), "style leaked: {parsed:?}");
        assert!(!parsed.contains("bad()"), "script leaked: {parsed:?}");
    }

    #[test]
    fn html_excludes_head_and_title_text() {
        let html = "<html><head><title>页面标题</title></head><body><p>正文</p></body></html>";
        let parsed = parse_text(html, DocumentFormat::Html).expect("html parses");
        assert!(
            !parsed.contains("页面标题"),
            "title leaked into body text: {parsed:?}"
        );
        assert!(parsed.contains("正文"), "missing body text: {parsed:?}");

        let title_only = "<html><head><title>页面标题</title></head><body></body></html>";
        let result = parse_text(title_only, DocumentFormat::Html);
        assert!(
            matches!(result, Err(KbError::Validation(_))),
            "expected validation error for title-only page, got {result:?}"
        );
    }

    #[test]
    fn html_keeps_inline_text_after_headings_and_blocks_on_separate_lines() {
        let parsed = parse_text("<h1>Title</h1>Intro", DocumentFormat::Html).expect("html parses");
        assert_eq!(parsed, "# Title\nIntro");

        let parsed = parse_text("<p>Hello</p>World", DocumentFormat::Html).expect("html parses");
        assert_eq!(parsed, "Hello\nWorld");
    }

    #[test]
    fn json_flattens_string_leaves() {
        let parsed = parse_text(
            r#"{"a":"foo","b":["bar"],"c":{"d":"baz"}}"#,
            DocumentFormat::Json,
        )
        .expect("json parses");
        let lines: Vec<&str> = parsed.lines().collect();
        for expected in ["foo", "bar", "baz"] {
            assert!(
                lines.contains(&expected),
                "missing string leaf {expected:?} in {lines:?}"
            );
        }
    }

    #[test]
    fn json_extracts_text_field_arrays() {
        let parsed = parse_text(r#"[{"text":"a"},{"text":"b"}]"#, DocumentFormat::Json)
            .expect("json parses");
        assert_eq!(parsed, "a\nb");
    }

    #[test]
    fn decodes_gbk_and_rejects_invalid_bytes() {
        let (gbk_bytes, _, _) = encoding_rs::GBK.encode("中文");
        assert_eq!(
            decode_bytes(&gbk_bytes, Some("gbk")).expect("gbk decodes"),
            "中文"
        );

        let result = decode_bytes(&[0xFF, 0xFE], None);
        assert!(
            matches!(result, Err(KbError::InvalidEncoding(_))),
            "expected invalid encoding, got {result:?}"
        );
    }

    #[test]
    fn decodes_utf8_bom_and_gb18030_and_rejects_unknown_labels() {
        let mut bom_input = UTF8_BOM.to_vec();
        bom_input.extend_from_slice("中文".as_bytes());
        assert_eq!(
            decode_bytes(&bom_input, None).expect("utf-8 decodes"),
            "中文"
        );

        let (gb18030_bytes, _, _) = encoding_rs::GB18030.encode("中文");
        assert_eq!(
            decode_bytes(&gb18030_bytes, Some("gb18030")).expect("gb18030 decodes"),
            "中文"
        );

        let result = decode_bytes(b"hello", Some("latin1"));
        assert!(
            matches!(result, Err(KbError::InvalidEncoding(_))),
            "expected invalid encoding, got {result:?}"
        );
    }

    #[test]
    fn blank_document_is_validation_error() {
        let result = parse_text("  \n\t ", DocumentFormat::Text);
        assert!(
            matches!(result, Err(KbError::Validation(_))),
            "expected validation error, got {result:?}"
        );
    }
}
