//! Diagnostics use fixed samples; they never read or write knowledge-base data.

use std::time::Instant;

use kb_core::error::{KbError, Result};
use kb_core::ingest::parser::parse_text;
use kb_core::ingest::upload::{ParsingProvider, UploadInput};
use kb_core::rerank::RerankProvider;
use kb_core::service::KnowledgeBaseService;
use serde::Serialize;

#[derive(Serialize)]
pub(crate) struct RerankProbe {
    provider: String,
    model: String,
    scored: usize,
    latency_ms: u64,
}

pub(super) async fn rerank(
    reranker: &dyn RerankProvider,
    provider: String,
    model: String,
) -> Result<RerankProbe> {
    let started = Instant::now();
    let ranked = reranker
        .rerank(
            "What does a library lend to visitors?",
            &[
                "The library lends books to visitors.",
                "Rain is expected in the afternoon.",
            ],
        )
        .await?;
    Ok(RerankProbe {
        provider,
        model,
        scored: ranked.len(),
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

#[derive(Serialize)]
pub(crate) struct ParsingProbe {
    provider: ParsingProvider,
    fixture: &'static str,
    characters: usize,
    latency_ms: u64,
}

pub(super) async fn parsing(
    service: &KnowledgeBaseService,
    provider: ParsingProvider,
    cloud_model: &str,
) -> Result<ParsingProbe> {
    let (fixture, bytes, parser) = match provider {
        ParsingProvider::Native => (
            "sample.md",
            b"# Parsing test\n\nThis is a sample document.".to_vec(),
            None,
        ),
        ParsingProvider::MineruCloud if cloud_model == "MinerU-HTML" => (
            "sample.html",
            b"<!doctype html><html><body><h1>Parsing test</h1><p>This is a sample document.</p></body></html>"
                .to_vec(),
            Some(ParsingProvider::MineruCloud),
        ),
        _ => ("sample.pdf", sample_pdf(), None),
    };
    let started = Instant::now();
    let prepared = service
        .prepare_upload(UploadInput {
            filename: fixture.into(),
            bytes,
            doc_id: None,
            format: None,
            encoding: None,
            parser,
            metadata: Default::default(),
        })
        .await?;
    let text = parse_text(&prepared.text, prepared.format)?;
    let characters = text.chars().filter(|ch| !ch.is_whitespace()).count();
    if characters == 0 {
        return Err(KbError::ParserUnavailable(
            "the parser returned no text for the sample document".into(),
        ));
    }
    Ok(ParsingProbe {
        provider,
        fixture,
        characters,
        latency_ms: started.elapsed().as_millis() as u64,
    })
}

/// Small valid one-page PDF with byte offsets computed from the emitted bytes.
fn sample_pdf() -> Vec<u8> {
    use std::io::Write;

    let stream = "BT /F1 18 Tf 54 722 Td (Knowledge base parser test) Tj 0 -28 Td (This is a sample document.) Tj ET\n";
    let content = format!("<< /Length {} >>\nstream\n{stream}endstream", stream.len());
    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
        &content,
    ];
    let mut bytes = b"%PDF-1.4\n".to_vec();
    let mut offsets = Vec::with_capacity(objects.len());
    for (index, object) in objects.iter().enumerate() {
        offsets.push(bytes.len());
        writeln!(bytes, "{} 0 obj\n{object}\nendobj", index + 1).expect("write into memory");
    }
    let xref = bytes.len();
    let size = offsets.len() + 1;
    writeln!(bytes, "xref\n0 {size}\n0000000000 65535 f ").expect("write into memory");
    for offset in offsets {
        writeln!(bytes, "{offset:010} 00000 n ").expect("write into memory");
    }
    writeln!(
        bytes,
        "trailer\n<< /Size {size} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF"
    )
    .expect("write into memory");
    bytes
}
