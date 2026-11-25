//! Heading-aware document chunking.
//!
//! The chunker works on the normalized plain text produced by the parsers. For
//! Markdown and HTML inputs, Markdown-style `#` lines delimit sections and build
//! a `heading_path`; for plain text and JSON every line is content. All size
//! limits are measured in Unicode scalar values (chars), never bytes or tokens.

use std::collections::VecDeque;

use crate::config::IngestConfig;
use crate::error::{KbError, Result};
use crate::models::DocumentFormat;

/// A contiguous slice of a document, tagged with the heading path it belongs to.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Chunk {
    pub index: usize,
    pub text: String,
    pub heading_path: String,
}

/// Characters that end a sentence; the terminator stays with the sentence.
const SENTENCE_TERMINATORS: [char; 7] = ['。', '！', '？', '!', '?', '.', '\n'];

/// Splits `text` into heading-aware chunks of at most `config.chunk_size` chars.
///
/// Only [`DocumentFormat::Markdown`] and [`DocumentFormat::Html`] interpret `#`
/// heading lines; the HTML parser already emits such lines. Heading lines are
/// never part of a chunk's text. Chunks carry the trailing `chunk_overlap`
/// characters of the previous chunk, and a final chunk shorter than
/// `min_chunk_size` is merged into its predecessor when that stays within
/// `chunk_size + chunk_overlap`.
pub fn chunk_document(
    text: &str,
    format: DocumentFormat,
    config: &IngestConfig,
) -> Result<Vec<Chunk>> {
    if text.trim().is_empty() {
        return Err(KbError::Validation(
            "document text must not be blank".to_owned(),
        ));
    }
    if config.chunk_size == 0 {
        return Err(KbError::Validation(
            "ingest.chunk_size must be greater than 0".to_owned(),
        ));
    }
    if config.chunk_overlap >= config.chunk_size {
        return Err(KbError::Validation(
            "ingest.chunk_overlap must be smaller than ingest.chunk_size".to_owned(),
        ));
    }

    let normalized = text.replace("\r\n", "\n").replace('\r', "\n");
    let headings_enabled = matches!(format, DocumentFormat::Markdown | DocumentFormat::Html);
    let sections = split_sections(&normalized, headings_enabled);

    let mut chunks = Vec::new();
    for section in sections {
        chunks.extend(chunk_section(&section.body, &section.heading_path, config));
    }

    if chunks.is_empty() {
        return Err(KbError::Validation(
            "document produced no chunks".to_owned(),
        ));
    }
    for (index, chunk) in chunks.iter_mut().enumerate() {
        chunk.index = index;
    }
    Ok(chunks)
}

/// A heading-delimited slice of the document.
struct Section {
    heading_path: String,
    body: String,
}

/// Splits the document at Markdown heading lines.
///
/// The heading stack tracks the current path; a heading at the same or a
/// shallower level pops deeper entries before being pushed.
fn split_sections(text: &str, headings_enabled: bool) -> Vec<Section> {
    if !headings_enabled {
        let body = text.trim();
        if body.is_empty() {
            return Vec::new();
        }
        return vec![Section {
            heading_path: String::new(),
            body: body.to_owned(),
        }];
    }

    let mut sections = Vec::new();
    let mut stack: Vec<(usize, String)> = Vec::new();
    let mut heading_path = String::new();
    let mut body = String::new();

    for line in text.lines() {
        if let Some((level, title)) = parse_heading(line) {
            push_section(&mut sections, &heading_path, &mut body);
            while stack
                .last()
                .is_some_and(|(stack_level, _)| *stack_level >= level)
            {
                stack.pop();
            }
            stack.push((level, title));
            heading_path = stack
                .iter()
                .map(|(_, title)| title.as_str())
                .collect::<Vec<_>>()
                .join(" > ");
        } else {
            body.push_str(line);
            body.push('\n');
        }
    }
    push_section(&mut sections, &heading_path, &mut body);
    sections
}

/// Flushes the accumulated body when it holds non-whitespace text.
fn push_section(sections: &mut Vec<Section>, heading_path: &str, body: &mut String) {
    let trimmed = body.trim();
    if !trimmed.is_empty() {
        sections.push(Section {
            heading_path: heading_path.to_owned(),
            body: trimmed.to_owned(),
        });
    }
    body.clear();
}

/// Parses an ATX heading line (`#` .. `######` plus a title).
fn parse_heading(line: &str) -> Option<(usize, String)> {
    let trimmed = line.trim_start();
    let level = trimmed.chars().take_while(|&ch| ch == '#').count();
    if level == 0 || level > 6 {
        return None;
    }
    // `#` is ASCII, so the level is also a valid byte offset.
    let rest = &trimmed[level..];
    if let Some(first) = rest.chars().next() {
        if !first.is_whitespace() {
            return None;
        }
    }
    Some((level, rest.trim().to_owned()))
}

/// A chunk under construction plus the length of its carried overlap prefix.
struct RawChunk {
    text: String,
    heading_path: String,
    overlap_prefix_len: usize,
}

/// Chunks one section. Overlap is reset at section boundaries so adjacent
/// sections with different heading paths never blend into a chunk.
fn chunk_section(body: &str, heading_path: &str, config: &IngestConfig) -> Vec<Chunk> {
    let mut pending = build_pieces(body, config.chunk_size);
    let mut raw_chunks: Vec<RawChunk> = Vec::new();
    let mut previous: Option<String> = None;

    while !pending.is_empty() {
        let mut current: Vec<char> = Vec::new();
        let overlap_prefix_len = match &previous {
            Some(previous_text) => {
                let previous_chars: Vec<char> = previous_text.chars().collect();
                let take = config.chunk_overlap.min(previous_chars.len());
                current.extend_from_slice(&previous_chars[previous_chars.len() - take..]);
                take
            }
            None => 0,
        };

        while let Some(front) = pending.front() {
            let capacity = config.chunk_size.saturating_sub(current.len());
            if capacity == 0 {
                break;
            }
            let front_len = front.chars().count();
            if front_len <= capacity {
                current.extend(front.chars());
                pending.pop_front();
                continue;
            }

            // Prefer starting the next chunk with this whole piece: overlap
            // plus the piece must fit within a single chunk.
            let next_overlap = config.chunk_overlap.min(current.len());
            if current.len() > overlap_prefix_len && front_len + next_overlap <= config.chunk_size {
                break;
            }

            let mut front_chars = front.chars();
            for _ in 0..capacity {
                if let Some(character) = front_chars.next() {
                    current.push(character);
                }
            }
            let remainder: String = front_chars.collect();
            pending.pop_front();
            if !remainder.is_empty() {
                pending.push_front(remainder);
            }
            break;
        }

        let text = current
            .into_iter()
            .collect::<String>()
            .trim_end()
            .to_owned();
        if text.is_empty() {
            continue;
        }
        previous = Some(text.clone());
        raw_chunks.push(RawChunk {
            text,
            heading_path: heading_path.to_owned(),
            overlap_prefix_len,
        });
    }

    merge_tiny_tail(raw_chunks, config)
}

/// Builds the ordered piece stream for a section: paragraphs separated by a
/// blank line, each paragraph split into sentence-sized and hard-split pieces.
fn build_pieces(body: &str, chunk_size: usize) -> VecDeque<String> {
    let mut pieces = VecDeque::new();
    for (index, paragraph) in split_paragraphs(body).iter().enumerate() {
        if index > 0 {
            pieces.push_back("\n\n".to_owned());
        }
        pieces.extend(split_into_pieces(paragraph, chunk_size));
    }
    pieces
}

/// Splits a section body into paragraphs on blank lines. Single newlines stay
/// inside a paragraph.
fn split_paragraphs(body: &str) -> Vec<String> {
    let mut paragraphs = Vec::new();
    let mut current = String::new();
    for line in body.lines() {
        if line.trim().is_empty() {
            push_paragraph(&mut paragraphs, &mut current);
        } else {
            if !current.is_empty() {
                current.push('\n');
            }
            current.push_str(line.trim_end());
        }
    }
    push_paragraph(&mut paragraphs, &mut current);
    paragraphs
}

fn push_paragraph(paragraphs: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim();
    if !trimmed.is_empty() {
        paragraphs.push(trimmed.to_owned());
    }
    current.clear();
}

/// Splits a paragraph into pieces. Paragraphs at or below `chunk_size` stay
/// whole; longer ones split after sentence terminators and any still-oversized
/// sentence is hard-split by Unicode scalar values.
fn split_into_pieces(paragraph: &str, chunk_size: usize) -> Vec<String> {
    if paragraph.chars().count() <= chunk_size {
        return vec![paragraph.to_owned()];
    }

    let mut sentences = Vec::new();
    let mut current = String::new();
    for character in paragraph.chars() {
        current.push(character);
        if SENTENCE_TERMINATORS.contains(&character) {
            sentences.push(std::mem::take(&mut current));
        }
    }
    if !current.is_empty() {
        sentences.push(current);
    }

    let mut pieces = Vec::new();
    for sentence in sentences {
        split_by_chars(&sentence, chunk_size, &mut pieces);
    }
    pieces
}

fn split_by_chars(text: &str, max: usize, pieces: &mut Vec<String>) {
    let mut current = String::new();
    let mut count = 0;
    for character in text.chars() {
        current.push(character);
        count += 1;
        if count == max {
            pieces.push(std::mem::take(&mut current));
            count = 0;
        }
    }
    if !current.is_empty() {
        pieces.push(current);
    }
}

/// Merges a final undersized chunk into its predecessor when the result stays
/// within `chunk_size + chunk_overlap`. The carried overlap prefix is removed
/// first so the merge does not duplicate it.
fn merge_tiny_tail(mut raw_chunks: Vec<RawChunk>, config: &IngestConfig) -> Vec<Chunk> {
    if raw_chunks.len() >= 2 {
        let final_len = raw_chunks
            .last()
            .map_or(0, |chunk| chunk.text.chars().count());
        if final_len < config.min_chunk_size {
            let previous_len = raw_chunks[raw_chunks.len() - 2].text.chars().count();
            let last = raw_chunks.last().expect("checked non-empty");
            let carried = last.overlap_prefix_len.min(final_len);
            let merged_len = previous_len + (final_len - carried);
            if merged_len <= config.chunk_size.saturating_add(config.chunk_overlap) {
                let final_chunk = raw_chunks.pop().expect("checked non-empty");
                let remaining: String = final_chunk.text.chars().skip(carried).collect();
                let previous = raw_chunks.last_mut().expect("checked non-empty");
                previous.text.push_str(&remaining);
                previous.text = previous.text.trim_end().to_owned();
            }
        }
    }

    raw_chunks
        .into_iter()
        .enumerate()
        .map(|(index, chunk)| Chunk {
            index,
            text: chunk.text,
            heading_path: chunk.heading_path,
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::IngestConfig;
    use crate::error::KbError;
    use crate::models::DocumentFormat;

    fn config(chunk_size: usize, chunk_overlap: usize, min_chunk_size: usize) -> IngestConfig {
        IngestConfig {
            chunk_size,
            chunk_overlap,
            min_chunk_size,
            upsert_batch_size: 64,
        }
    }

    fn tail(text: &str, count: usize) -> String {
        let chars: Vec<char> = text.chars().collect();
        let start = chars.len().saturating_sub(count);
        chars[start..].iter().collect()
    }

    #[test]
    fn short_text_is_one_chunk() {
        let text = "01234567890123456789";
        let chunks = chunk_document(text, DocumentFormat::Text, &config(100, 0, 5))
            .expect("short text chunks");
        assert_eq!(chunks.len(), 1);
        assert_eq!(chunks[0].index, 0);
        assert_eq!(chunks[0].text, text);
        assert_eq!(chunks[0].heading_path, "");
    }

    #[test]
    fn long_chinese_text_splits_within_limit_and_overlaps() {
        let text = "中".repeat(60);
        let chunks =
            chunk_document(&text, DocumentFormat::Text, &config(20, 5, 5)).expect("text chunks");
        assert!(chunks.len() > 1, "expected multiple chunks, got {chunks:?}");
        for chunk in &chunks {
            assert!(
                chunk.text.chars().count() <= 20,
                "chunk exceeds the limit: {chunk:?}"
            );
        }
        for pair in chunks.windows(2) {
            let expected_prefix = tail(&pair[0].text, 5);
            assert!(
                pair[1].text.starts_with(&expected_prefix),
                "{:?} does not start with the last five chars of {:?}",
                pair[1],
                pair[0]
            );
        }
    }

    #[test]
    fn markdown_headings_become_heading_path() {
        let text = "# 第一章\n\n内容A\n\n## 小节\n\n内容B";
        let chunks = chunk_document(text, DocumentFormat::Markdown, &config(100, 0, 0))
            .expect("markdown chunks");
        assert_eq!(chunks.len(), 2);
        assert_eq!(chunks[0].heading_path, "第一章");
        assert_eq!(chunks[1].heading_path, "第一章 > 小节");
        assert_eq!(chunks[0].text, "内容A");
        assert_eq!(chunks[1].text, "内容B");
        for chunk in &chunks {
            assert!(
                !chunk.text.contains("# 第一章"),
                "heading line leaked into chunk: {chunk:?}"
            );
            assert!(
                !chunk.text.contains("## 小节"),
                "heading line leaked into chunk: {chunk:?}"
            );
        }
    }

    #[test]
    fn tiny_tail_is_merged() {
        let text = "0123456789012345678901";
        assert_eq!(text.chars().count(), 22);
        let chunks =
            chunk_document(text, DocumentFormat::Text, &config(20, 5, 10)).expect("text chunks");
        assert_eq!(chunks.len(), 1, "tiny tail was not merged: {chunks:?}");
        assert_eq!(chunks[0].text, text);
    }

    #[test]
    fn empty_or_blank_is_validation_error() {
        let result = chunk_document("   ", DocumentFormat::Text, &config(20, 5, 5));
        assert!(
            matches!(result, Err(KbError::Validation(_))),
            "expected validation error, got {result:?}"
        );
    }
}
