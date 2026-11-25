# Qdrant Knowledge Base Service Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Build a Rust workspace (`kb-core` + `kb-server`) that exposes a REST knowledge-base service backed by Qdrant, with pluggable local (fastembed/ONNX) and OpenAI-compatible API embeddings, plus a complete Chinese README.

**Architecture:** `kb-core` owns config, domain models, embedding trait + two providers, parsing/chunking, Qdrant access, and the use-case service. `kb-server` is a thin axum layer (auth, limits, error mapping, HTTP handlers). Writes are single-instance, serialized per `collection + doc_id` with striped locks; ingestion reconciles stale points so retries self-heal.

**Tech Stack:** Rust 1.100 (edition 2021, MSRV 1.92), `qdrant-client` 1.20.0 (gRPC 6334), `fastembed` 7.8.0 (ONNX), `axum` 0.8.9, `figment` 0.10.19, `tokio`, `reqwest`, `serde`, `uuid` v5, `sha2`, `scraper`, `encoding_rs`.

**Spec:** `docs/superpowers/specs/2026-11-25-qdrant-knowledge-base-design.md`

## Global Constraints

- Workspace members: `crates/kb-core`, `crates/kb-server`; dependency direction is `kb-server -> kb-core` only. `kb-core` must not depend on axum.
- Exact versions where verified: `qdrant-client = "1.20.0"`, `fastembed = "7.8.0"` (optional), `axum = "0.8.9"`, `figment = "0.10.19"`. Other crates: latest stable resolved by `cargo add`, locked in `Cargo.lock`.
- Features: `kb-core` has `default = ["local-embedding"]`, `local-embedding = ["dep:fastembed"]`. `kb-server` mirrors `local-embedding = ["kb-core/local-embedding"]` and keeps it in default. `DeterministicEmbedding` is always compiled (small test helper).
- Qdrant is remote-only (no embedded mode); gRPC URL default `http://localhost:6334`; all collections use Cosine distance; every write uses `wait(true)`.
- Config: built-in defaults, then optional `config/config.toml`, then environment variables prefixed `KB_` with `__` as nesting separator. Config file path overridable by `KB_CONFIG` or `--config <path>`.
- REST prefix `/v1`; error envelope is exactly `{"error":{"code":"...","message":"..."}}`. `/health` requires no auth.
- Storage identity: `content_hash = sha256(normalized_text)`; point id = UUID v5 over `{collection}:{doc_id}:{content_hash}:{chunk_index}`; every ingest reconciles stale points by `must_not has_id(new_ids)`.
- Single-instance deployment. In-process 64-stripe locks serialize ingest/delete per `collection + doc_id`. README must state this.
- Upload decoding: UTF-8 by default (BOM stripped); explicit `encoding=gbk|gb18030` supported via `encoding_rs`; no auto-detection.
- Tests that need Qdrant use `KB_TEST_QDRANT_URL`; when unset they print a skip line and return. Unit tests must never require network or a model download.
- Local embedding is compile-time optional; `cargo build --no-default-features -p kb-server` must compile an API-only server (choosing `provider=local` at runtime then returns a clear config error).
- Every task ends with `cargo fmt`, `cargo clippy --all-targets -- -D warnings` for touched crates, tests, and a git commit. All commits end with `Co-Authored-By: Claude Code <noreply@anthropic.com>`.
- README is Chinese; public API doc comments are English; inline comments may be Chinese or English.

## Review Focus

These are the failure modes most likely to bite a real user; each line's test lives in the named task.

1. **Concurrent ingest of the same `doc_id` with different content** — one interleaving of upsert-then-cleanup can erase the document. Test: Task 6 `concurrent_ingests_of_same_doc_leave_one_complete_version`.
2. **Retry after a partial upsert or stale same-hash tail** — the skip path must repair, not return `skipped=true` over missing chunks. Test: Task 6 `reinject_repairs_missing_points_and_stale_tail`.
3. **Non-UTF-8 and empty documents** — GBK/GB18030 must decode when declared; invalid bytes and blank documents must produce 4xx, never panic. Tests: Task 4 `decodes_gbk_and_rejects_invalid_bytes`, Task 4 `blank_document_is_validation_error`.
4. **Unicode/slash-containing `doc_id` and missing collections** — delete must work via query parameter; search/ingest on missing collections must be 404, not a Qdrant 500. Tests: Task 6 `delete_document_with_unicode_and_slash_ids`, Task 6 `missing_collection_maps_to_not_found`.
5. **Timeout in the middle of ingest** — the HTTP layer must return the envelope with 408 and a retry must converge to a complete document. Tests: Task 7 `timeout_returns_408_envelope`, Task 6 `reinject_after_partial_write_converges`.

---

## File Structure

```
D:\RAG\
├── Cargo.toml                         # workspace
├── Cargo.lock
├── .gitignore
├── .env.example
├── Dockerfile
├── docker-compose.yml
├── README.md
├── config/config.example.toml
├── scripts/start-qdrant.ps1           # Windows dev helper: download/start Qdrant
├── crates/kb-core/
│   ├── Cargo.toml
│   ├── src/lib.rs
│   ├── src/config.rs
│   ├── src/error.rs
│   ├── src/models.rs
│   ├── src/embedding/mod.rs           # trait
│   ├── src/embedding/api.rs           # OpenAI-compatible
│   ├── src/embedding/local.rs         # fastembed
│   ├── src/embedding/mock.rs          # deterministic test/demo provider
│   ├── src/ingest/mod.rs
│   ├── src/ingest/parser.rs
│   ├── src/ingest/chunker.rs
│   ├── src/store.rs                   # QdrantStore
│   ├── src/service.rs                 # KnowledgeBaseService + striped locks
│   └── tests/store_service.rs         # env-gated integration tests
└── crates/kb-server/
    ├── Cargo.toml
    ├── src/main.rs
    ├── src/state.rs
    ├── src/error.rs                   # ApiError -> HTTP
    ├── src/api/mod.rs
    ├── src/api/routes.rs
    ├── src/api/handlers.rs
    └── tests/api.rs                   # oneshot HTTP tests
```

---

## Execution Waves

- Wave 0 (serial): Task 1.
- Wave 1 (parallel, disjoint files): Tasks 2, 3, 4, 5.
- Wave 2: Task 6 (needs Tasks 4 and 5; may overlap the tail of 2/3).
- Wave 3: Task 7 (needs Tasks 2, 3, 6).
- Wave 4: Task 8.
- Wave 5: Task 9 (final verification).

---

### Task 1: Workspace, config, errors, domain models, embedding trait, mock provider

**Files:**
- Create: `Cargo.toml`, `.gitignore`, `config/config.example.toml`
- Create: `crates/kb-core/Cargo.toml`, `crates/kb-core/src/lib.rs`
- Create: `crates/kb-core/src/config.rs`, `error.rs`, `models.rs`
- Create: `crates/kb-core/src/embedding/mod.rs`, `embedding/api.rs` (empty stub), `embedding/local.rs` (empty stub), `embedding/mock.rs`
- Create: `crates/kb-core/src/ingest/mod.rs`, `ingest/parser.rs` (empty stub), `ingest/chunker.rs` (empty stub)
- Create: `crates/kb-core/src/store.rs` (empty stub), `service.rs` (empty stub)
- Create: `crates/kb-server/Cargo.toml`, `crates/kb-server/src/main.rs` (placeholder)

**Interfaces:**
- Produces:
  - `kb_core::config::{AppConfig, ServerConfig, QdrantConfig, EmbeddingConfig, ProviderKind, LocalEmbeddingConfig, ApiEmbeddingConfig, IngestConfig}` with `AppConfig::load(path: Option<&Path>) -> Result<AppConfig>`.
  - `kb_core::error::{KbError, Result}`; variants: `Config`, `Validation`, `InvalidEncoding`, `UnsupportedFormat`, `Unauthorized`, `CollectionNotFound`, `CollectionExists`, `DimensionMismatch`, `Embedding`, `Qdrant`, `Parse`, `Io`, `Internal`.
  - `kb_core::models::{DocumentFormat, DocumentMetadata, IngestDocumentRequest, IngestResult, SearchRequest, SearchFilter, SearchHitMetadata, SearchHit, CollectionInfo, HealthStatus, EmbeddingInfo}`.
  - `kb_core::embedding::{EmbeddingProvider, mock::DeterministicEmbedding}`.
- Consumes: nothing.

- [ ] **Step 1: Write the failing tests**

In `config.rs` tests:

```rust
#[test]
#[serial_test::serial]
fn env_overrides_toml_and_defaults() {
    let path = std::env::temp_dir().join(format!("kb-config-{}.toml", uuid::Uuid::new_v4()));
    std::fs::write(&path, r#"
[server]
port = 9000

[embedding]
provider = "api"

[embedding.api]
base_url = "https://example.com/v1"
model = "embed-1"
"#).unwrap();
    std::env::set_var("KB_SERVER__PORT", "9100");
    std::env::set_var("KB_EMBEDDING__API__API_KEY", "sk-test");
    let cfg = AppConfig::load(Some(path.as_path())).unwrap();
    assert_eq!(cfg.server.port, 9100);
    assert_eq!(cfg.embedding.provider, ProviderKind::Api);
    assert_eq!(cfg.embedding.api.api_key, "sk-test");
    assert_eq!(cfg.ingest.chunk_size, 800);
    std::env::remove_var("KB_SERVER__PORT");
    std::env::remove_var("KB_EMBEDDING__API__API_KEY");
    let _ = std::fs::remove_file(path);
}

#[test]
fn rejects_overlap_not_smaller_than_chunk_size() {
    let mut cfg = AppConfig::default();
    cfg.ingest.chunk_overlap = cfg.ingest.chunk_size;
    assert!(matches!(cfg.validate(), Err(KbError::Config(_))));
}
```

In `models.rs` tests: `infer_from_filename` maps `.md/.markdown/.html/.htm/.json` and defaults to `Text`; `DocumentFormat::parse` accepts the four lowercase names and returns `KbError::UnsupportedFormat` for anything else.

In `embedding/mock.rs` tests: same text gives identical vector; `dimension()` matches vector length; different texts differ; `call_count()` increments once per `embed_documents`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core config::tests::env_overrides_toml_and_defaults`
Expected: compile error, `AppConfig` not found.

- [ ] **Step 3: Implement the workspace and shared types**

Create the workspace manifest with `members = ["crates/kb-core", "crates/kb-server"]`, `resolver = "2"`, and `[workspace.package] edition = "2021"`, `rust-version = "1.92"`.

`kb-core` dependencies: `async-trait`, `chrono` (serde), `encoding_rs`, `fastembed` (optional, version 7.8.0), `figment` (toml, env, version 0.10.19), `futures`, `qdrant-client` (version 1.20.0), `reqwest` (json, rustls-tls), `scraper` (resolve latest stable with `cargo add scraper`), `serde` (derive), `serde_json`, `sha2`, `thiserror`, `tokio` (full), `tracing`, `uuid` (v5). Dev-deps: `serial_test`, `wiremock`. Features exactly as in Global Constraints.

`kb-server` dependencies: `anyhow`, `axum` (multipart), `kb-core` with `default-features = false`, `serde`, `serde_json`, `tokio`, `tower`, `tower-http` (trace, timeout), `tracing`, `tracing-subscriber` (env-filter). Dev-deps: `http-body-util`, `tower` (util), `wiremock`, `serial_test`.

Config defaults (all strings trimmed; empty API key allowed):

| section.key | default |
|---|---|
| `server.host` | `0.0.0.0` |
| `server.port` | `8080` |
| `server.api_key` | `""` |
| `server.max_body_mb` | `32` |
| `server.request_timeout_secs` | `300` |
| `qdrant.url` | `http://localhost:6334` |
| `qdrant.api_key` | `""` |
| `qdrant.timeout_secs` | `30` |
| `qdrant.startup_retry_secs` | `30` |
| `embedding.provider` | `Local` |
| `embedding.batch_size` | `32` |
| `embedding.max_concurrent_batches` | `4` |
| `embedding.local.model` | `bge-small-zh-v1.5` |
| `embedding.local.cache_dir` | `./data/models` |
| `embedding.local.show_download_progress` | `true` |
| `embedding.local.max_length` | `512` |
| `embedding.local.warmup` | `true` |
| `embedding.local.query_prefix` | `""` |
| `embedding.local.document_prefix` | `""` |
| `embedding.api.base_url` | `https://api.openai.com/v1` |
| `embedding.api.api_key` | `""` |
| `embedding.api.model` | `text-embedding-3-small` |
| `embedding.api.dimensions` | `0` |
| `embedding.api.timeout_secs` | `60` |
| `embedding.api.max_retries` | `3` |
| `ingest.chunk_size` | `800` |
| `ingest.chunk_overlap` | `120` |
| `ingest.min_chunk_size` | `30` |
| `ingest.upsert_batch_size` | `64` |

`AppConfig::load`: start from `Serialized::defaults(AppConfig::default())`; if an explicit path (argument or `KB_CONFIG`) is given, merge `Toml::file_exact`; otherwise merge `Toml::file("config/config.toml")` only when that file exists. Then merge `Env::prefixed("KB_").split("__")`, extract, and call `validate()`. All config structs derive `Serialize, Deserialize`; `ProviderKind` and `DocumentFormat` use `#[serde(rename_all = "lowercase")]`.

`validate()` rejects: port 0; `max_body_mb`/timeouts/batch sizes of 0; empty Qdrant URL; `chunk_size == 0`; `chunk_overlap >= chunk_size`; `min_chunk_size > chunk_size`; empty local model when provider is local; empty API base URL/model when provider is api.

`models.rs` definitions exactly:

```rust
pub enum DocumentFormat { Text, Markdown, Html, Json }
impl DocumentFormat {
    pub fn infer_from_filename(name: &str) -> Self;
    pub fn parse(value: &str) -> Result<Self>; // unknown -> KbError::UnsupportedFormat
}

pub struct DocumentMetadata { pub source: Option<String>, pub title: Option<String>, pub tags: Vec<String>, pub extra: serde_json::Map<String, serde_json::Value> }
pub struct IngestDocumentRequest { pub doc_id: Option<String>, pub text: String, pub format: Option<String>, pub metadata: DocumentMetadata }
pub struct IngestResult { pub doc_id: String, pub chunks: usize, pub skipped: bool, pub metadata_updated: bool, pub content_hash: String, pub embedding_model: String }
pub struct SearchRequest { pub query: String, pub top_k: usize, pub score_threshold: Option<f32>, pub filter: Option<SearchFilter> }
pub struct SearchFilter { pub doc_ids: Option<Vec<String>>, pub tags_any: Option<Vec<String>>, pub source: Option<String> }
pub struct SearchHitMetadata { pub source: Option<String>, pub title: Option<String>, pub tags: Vec<String>, pub heading_path: String, pub extra: serde_json::Map<String, serde_json::Value> }
pub struct SearchHit { pub score: f32, pub doc_id: String, pub chunk_index: u64, pub text: String, pub metadata: SearchHitMetadata }
pub struct CollectionInfo { pub name: String, pub points_count: u64, pub vector_size: u64 }
pub struct HealthStatus { pub status: String, pub qdrant: String, pub embedding: EmbeddingInfo }
pub struct EmbeddingInfo { pub provider: String, pub model: String, pub dimension: usize }
```

`SearchRequest.top_k` defaults to 5 via serde default function; all optional metadata/filter fields default.

Embedding trait:

```rust
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimension(&self) -> usize;
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>>;
}
```

`mock.rs` (always compiled; documented as a test/demo helper): `DeterministicEmbedding::new(dim: usize)`; derives vectors by hashing each text with SHA-256 and expanding bytes into `f32` values in `[-1.0, 1.0]`, then L2-normalizing; `calls: AtomicUsize` increments once per `embed_documents` call; `embed_query` delegates to `embed_documents`.

Create empty module files for `api.rs`, `local.rs`, `parser.rs`, `chunker.rs`, `store.rs`, `service.rs`; `lib.rs` declares all modules and re-exports `KbError`/`Result`. `embedding/mod.rs` declares `pub mod api; pub mod mock;` and cfg-gates `pub mod local;`; `ingest/mod.rs` declares `pub mod parser; pub mod chunker;`. `local.rs` is compiled only with `#[cfg(feature = "local-embedding")]`.

Placeholder `kb-server/src/main.rs` loads config and prints host/port.

Write `config/config.example.toml` exactly matching the design's config block (with comments).

`.gitignore`: `/target`, `/data`, `/.tools`, `/config/config.toml`, `.env`, `*.log`, `.fastembed_cache`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-core`
Expected: all Task 1 tests PASS.

- [ ] **Step 5: Initialize git and commit**

If `git config user.email` is empty, set repo-local identity: `git config user.name "Claude Code"` and `git config user.email "noreply@anthropic.com"`. Then:

```bash
git init
git add Cargo.toml Cargo.lock .gitignore config crates docs
git commit -m "chore: scaffold kb workspace and shared core types" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 2: OpenAI-compatible embedding provider

**Files:**
- Modify: `crates/kb-core/src/embedding/api.rs`
- Test: inline `#[cfg(test)]` in the same file

**Interfaces:**
- Consumes: `EmbeddingProvider`, `ApiEmbeddingConfig`, `KbError`, `Result`.
- Produces: `pub struct ApiEmbedding` and `impl ApiEmbedding { pub async fn new(config: &ApiEmbeddingConfig, batch_size: usize, max_concurrent_batches: usize) -> Result<Self> }`; implements `EmbeddingProvider`. `model_id()` returns the configured model; `dimension()` returns the resolved dimension.

- [ ] **Step 1: Write the failing tests**

Use `wiremock`. Tests and assertions:

1. `embeds_batches_in_order_with_auth`: mount `POST /v1/embeddings`, require `Authorization: Bearer sk-test`, return `{"data":[{"index":1,"embedding":[0.0,1.0]},{"index":0,"embedding":[3.0,4.0]}]}`. Call `embed_documents(["a","b"])`; assert result is `[[3.0,4.0],[0.0,1.0]]`, `dimension()==2`.
2. `splits_into_batches`: `batch_size=2`, 5 inputs, mock `expect(3)`; assert 5 vectors returned.
3. `retries_on_429_then_succeeds`: first matching request returns 429 with `Retry-After: 0`, second returns 200; `max_retries=2`; assert success.
4. `non_2xx_is_embedding_error`: mock 500 always; assert `Err(KbError::Embedding(_))` and message contains `500`.
5. `probes_dimension_when_config_is_zero`: `dimensions=0`, mock returns 3-value vector; assert `dimension()==3`.
6. `rejects_wrong_dimension`: config `dimensions=2`, mock returns 3 values; assert `Err(KbError::Embedding(_))`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core embedding::api::tests::embeds_batches_in_order_with_auth`
Expected: compile error, `ApiEmbedding` not defined.

- [ ] **Step 3: Implement `ApiEmbedding`**

- Build a `reqwest::Client` with `timeout(config.timeout_secs)`.
- Trim trailing `/` from `base_url`; endpoint is `{base_url}/embeddings`.
- If `dimensions > 0`, use it; otherwise call `embed_documents(&["dimension probe".to_string()])` once during `new`, take the first vector length, and cache it. The probe uses the same retry path.
- Request body: `{"model": model, "input": texts}` plus `"dimensions"` only when non-zero.
- Headers: `Authorization: Bearer {api_key}` only when non-empty.
- Parse `{"data":[{"embedding":[...],"index":n}]}`; sort by `index`; validate count and every vector length equals the resolved dimension.
- Batch inputs by `batch_size`; run batches with `futures::stream::iter(...).buffer_unordered(max_concurrent_batches)`; collect `(batch_index, vectors)`, sort by batch index, flatten.
- Retry: up to `max_retries` extra attempts on 429, 5xx, and reqwest timeout/connect errors; sleep `min(250ms * 2^attempt, 5s)`; honor numeric `Retry-After` seconds; log attempts with `tracing::warn`.
- Non-2xx after retries: `KbError::Embedding(format!("embedding API returned {status}: {truncated_body}"))`.
- `embed_query` calls the same single-text path.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-core embedding::api`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kb-core/src/embedding/api.rs
git commit -m "feat: add OpenAI-compatible embedding provider" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 3: Local fastembed embedding provider

**Files:**
- Modify: `crates/kb-core/src/embedding/local.rs`
- Test: inline `#[cfg(test)]` in the same file

**Interfaces:**
- Consumes: `EmbeddingProvider`, `LocalEmbeddingConfig`, `KbError`, `Result`.
- Produces: `pub struct LocalEmbedding` and `impl LocalEmbedding { pub fn new(config: &LocalEmbeddingConfig, batch_size: usize) -> Result<Self> }`; `pub fn model_from_alias(alias: &str) -> Result<fastembed::EmbeddingModel>`; `pub fn default_prefixes(alias: &str) -> (String, String)`; `pub fn resolve_prefixes(config: &LocalEmbeddingConfig, alias: &str) -> (String, String)`. Implements `EmbeddingProvider`.

- [ ] **Step 1: Write the failing tests**

1. `maps_known_aliases`: `bge-small-zh-v1.5 -> BGESmallZHV15`, `bge-large-zh-v1.5 -> BGELargeZHV15`, `bge-m3 -> BGEM3`, `multilingual-e5-small/base/large`, `bge-small-en-v1.5 -> BGESmallENV15`, `all-minilm-l6-v2 -> AllMiniLML6V2`, `nomic-embed-text-v1.5 -> NomicEmbedTextV15`; unknown alias returns `KbError::Validation` whose message lists aliases.
2. `prefix_defaults_match_model_cards`: bge-small-zh query prefix is `为这个句子生成表示以用于检索相关文章：` and document prefix empty; `multilingual-e5-*` = `("query: ", "passage: ")`; `nomic-embed-text-*` = `("search_query: ", "search_document: ")`; `bge-m3` and `bge-small-en-v1.5` = `("", "")`.
3. `config_prefix_overrides_default`: non-empty `query_prefix`/`document_prefix` win.
4. `#[ignore] embeds_chinese_text_with_bge_small_zh`: construct with `cache_dir` = temp dir, `warmup=false`, embed `["今天天气怎么样", "年假怎么申请"]`; assert 2 vectors of length 512 and L2 norm within `1e-3` of 1.0.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core embedding::local::tests::maps_known_aliases`
Expected: compile error, `LocalEmbedding` not defined. (Unit tests 1–3 must not download a model.)

- [ ] **Step 3: Implement `LocalEmbedding`**

- `LocalEmbedding { model_alias: String, dimension: usize, batch_size: usize, query_prefix: String, document_prefix: String, model: Arc<std::sync::Mutex<Option<fastembed::TextEmbedding>>>, init: LocalInit }` where `LocalInit` is `Clone` and holds model enum, cache dir, download-progress flag, and max length.
- `new`: resolve alias; `dimension = TextEmbedding::get_model_info(&model)?.dim`; resolve prefixes with `resolve_prefixes(config, alias)` (config value if non-empty, else model default); if `warmup`, call `TextEmbedding::try_new(TextInitOptions::new(model).with_cache_dir(...).with_show_download_progress(...).with_max_length(...))` now and store `Some(model)`.
- Embedding call: prefix inputs; clone `Arc`, `LocalInit`, batch size; `tokio::task::spawn_blocking(move || { let mut guard = model.lock().unwrap(); if guard.is_none() { *guard = Some(init.load()?); } guard.as_mut().unwrap().embed(texts, Some(batch)) })`; map join errors and fastembed errors to `KbError::Embedding`.
- `embed_query` prefixes with `query_prefix`; `embed_documents` prefixes with `document_prefix`.
- Do not rely on fastembed for prefixes — it adds none.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-core embedding::local`
Expected: unit tests PASS; ignored model test not run.

- [ ] **Step 5: Commit**

```bash
git add crates/kb-core/src/embedding/local.rs
git commit -m "feat: add local fastembed embedding provider" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 4: Document parsers and text decoding

**Files:**
- Modify: `crates/kb-core/src/ingest/parser.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `DocumentFormat`, `KbError`, `Result`.
- Produces:
  - `pub fn decode_bytes(bytes: &[u8], encoding: Option<&str>) -> Result<String>`
  - `pub fn parse_text(text: &str, format: DocumentFormat) -> Result<String>`

- [ ] **Step 1: Write the failing tests**

1. `normalizes_text_and_markdown`: CRLF becomes LF; surrounding whitespace trimmed; Markdown heading text retained.
2. `html_extracts_visible_text_and_headings`: input `<html><head><style>p{}</style></head><body><h2>标题</h2><p>正文</p><script>bad()</script><ul><li>条目</li></ul></body></html>`; output contains `## 标题`, `正文`, `条目`; must not contain `p{}`, `bad()`.
3. `json_flattens_string_leaves`: `{"a":"foo","b":["bar"],"c":{"d":"baz"}}` yields lines containing `foo`, `bar`, `baz`.
4. `json_extracts_text_field_arrays`: `[{"text":"a"},{"text":"b"}]` yields `a` and `b` in order.
5. `decodes_gbk_and_rejects_invalid_bytes`: `encoding_rs::GBK.encode("中文").0` decoded with `Some("gbk")` equals `"中文"`; `vec![0xFF, 0xFE]` with `None` returns `Err(KbError::InvalidEncoding(_))`.
6. `blank_document_is_validation_error`: `parse_text("  \n\t ", DocumentFormat::Text)` returns `Err(KbError::Validation(_))`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core ingest::parser::tests::html_extracts_visible_text_and_headings`
Expected: compile error / unimplemented.

- [ ] **Step 3: Implement the parser**

- `decode_bytes`: `None`/`utf-8` strips a leading UTF-8 BOM and uses `str::from_utf8`; `gbk` and `gb18030` use `encoding_rs::GBK` / `encoding_rs::GB18030`; `had_errors` or unknown label → `KbError::InvalidEncoding`.
- `parse_text`: normalize CRLF to LF first.
  - Text/Markdown: return trimmed input.
  - Html: `scraper::Html::parse_document`; recursively walk nodes; skip `script/style/noscript`; emit `#` repeated 1–6 for `h1`–`h6` followed by a space; emit `- ` for `li`; emit newlines for block elements (`p`, `div`, `br`, `section`, `article`, `header`, `footer`, `ul`, `ol`, `table`, `tr`); append text-node content; collapse 3+ newlines to 2 and trim.
  - Json: parse with `serde_json`; if an array of objects with a string `text` field, join those in order; else if array of strings, join with `\n`; else recurse and collect every string leaf. No strings → `KbError::Validation`.
  - Empty/whitespace result → `KbError::Validation("document contains no text")`.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-core ingest::parser`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kb-core/src/ingest/parser.rs
git commit -m "feat: add text, markdown, html and json parsers with encoding support" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 5: Heading-aware chunker

**Files:**
- Modify: `crates/kb-core/src/ingest/chunker.rs`
- Test: inline `#[cfg(test)]`

**Interfaces:**
- Consumes: `IngestConfig`, `DocumentFormat`, `KbError`, `Result`.
- Produces:
  - `pub struct Chunk { pub index: usize, pub text: String, pub heading_path: String }` (derive `Debug, Clone, PartialEq, Eq`)
  - `pub fn chunk_document(text: &str, format: DocumentFormat, config: &IngestConfig) -> Result<Vec<Chunk>>`

- [ ] **Step 1: Write the failing tests**

1. `short_text_is_one_chunk`: 20-char text with `chunk_size=100` → 1 chunk, `index==0`, empty `heading_path`.
2. `long_chinese_text_splits_within_limit_and_overlaps`: `chunk_size=20`, `chunk_overlap=5`, `min_chunk_size=5`, 60 Chinese chars with no whitespace → more than one chunk; every chunk `chars().count() <= 20`; each chunk after the first starts with the last 5 chars of the previous chunk.
3. `markdown_headings_become_heading_path`: `# 第一章\n\n内容A\n\n## 小节\n\n内容B` with `chunk_size=100` → first chunk heading_path `第一章`, second `第一章 > 小节`; chunk texts do not contain the heading lines.
4. `tiny_tail_is_merged`: `chunk_size=20`, 22 chars of content → one chunk.
5. `empty_or_blank_is_validation_error`: `chunk_document("   ", Text, &cfg)` → `Err(KbError::Validation(_))`.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core ingest::chunker::tests::long_chinese_text_splits_within_limit_and_overlaps`
Expected: compile error / unimplemented.

- [ ] **Step 3: Implement the chunker**

- Only Markdown and Html formats interpret `#` headings; Text/Json treat everything as content. (Html parser already emits `#` lines.)
- Split into sections at heading lines, maintaining a stack of `(level, title)` and producing `heading_path = titles.join(" > ")`. Heading lines themselves are not chunk content.
- Within a section: split into paragraphs on blank lines; split paragraphs longer than `chunk_size` at sentence terminators `。！？!?.\n` while keeping the terminator; hard-split any remaining piece by Unicode scalar values.
- Pack pieces greedily into chunks of at most `chunk_size` chars; carry the trailing `chunk_overlap` chars of the previous chunk into the next (only when the next chunk would otherwise start a new piece); skip empty results. If the final chunk is shorter than `min_chunk_size` and merging it keeps the previous chunk at or below `chunk_size + chunk_overlap`, merge.
- Trim chunk ends without dropping the carried overlap in the middle of a document.
- Assign sequential `index` values across sections; return `KbError::Validation` if no chunks.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-core ingest::chunker`
Expected: PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kb-core/src/ingest/chunker.rs
git commit -m "feat: add heading-aware chunker with overlap" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 6: Qdrant store and KnowledgeBaseService

**Files:**
- Modify: `crates/kb-core/src/store.rs`, `crates/kb-core/src/service.rs`
- Create: `crates/kb-core/tests/store_service.rs`
- Create: `scripts/start-qdrant.ps1`

**Interfaces:**
- Consumes: Task 2–5 providers/parsers/chunker, `EmbeddingProvider`, config structs, models.
- Produces:
  - `pub struct QdrantStore` with:
    - `pub fn new(config: &QdrantConfig) -> Result<Self>`
    - `pub async fn wait_ready(&self, retry: std::time::Duration) -> Result<()>`
    - `pub async fn create_collection(&self, name: &str, dim: u64) -> Result<()>`
    - `pub async fn delete_collection(&self, name: &str) -> Result<()>`
    - `pub async fn collection_exists(&self, name: &str) -> Result<bool>`
    - `pub async fn list_collections(&self) -> Result<Vec<CollectionInfo>>`
    - `pub async fn collection_info(&self, name: &str) -> Result<CollectionInfo>`
    - `pub async fn upsert(&self, collection: &str, points: Vec<qdrant_client::qdrant::PointStruct>) -> Result<()>`
    - `pub async fn delete_by_filter(&self, collection: &str, filter: qdrant_client::qdrant::Filter) -> Result<()>`
    - `pub async fn count(&self, collection: &str, filter: Option<qdrant_client::qdrant::Filter>) -> Result<u64>`
    - `pub async fn scroll_payloads(&self, collection: &str, filter: qdrant_client::qdrant::Filter, limit: u32) -> Result<Vec<serde_json::Map<String, serde_json::Value>>>`
    - `pub async fn scroll_ids(&self, collection: &str, filter: qdrant_client::qdrant::Filter, limit: u32) -> Result<Vec<uuid::Uuid>>`
    - `pub async fn set_metadata_payload(&self, collection: &str, filter: qdrant_client::qdrant::Filter, payload: serde_json::Map<String, serde_json::Value>) -> Result<()>`
    - `pub async fn search(&self, collection: &str, vector: Vec<f32>, limit: u64, threshold: Option<f32>, filter: Option<qdrant_client::qdrant::Filter>) -> Result<Vec<qdrant_client::qdrant::ScoredPoint>>`
  - `pub struct IngestInput { pub doc_id: Option<String>, pub text: String, pub format: DocumentFormat, pub metadata: DocumentMetadata }`
  - `pub struct KnowledgeBaseService` with `new(store: Arc<QdrantStore>, embedder: Arc<dyn EmbeddingProvider>, ingest: IngestConfig) -> Self` and async methods:
    - `health(&self) -> HealthStatus`
    - `list_collections(&self) -> Result<Vec<CollectionInfo>>`
    - `create_collection(&self, name: &str) -> Result<CollectionInfo>`
    - `delete_collection(&self, name: &str) -> Result<()>`
    - `ingest_document(&self, collection: &str, input: IngestInput) -> Result<IngestResult>`
    - `delete_document(&self, collection: &str, doc_id: &str) -> Result<u64>`
    - `search(&self, collection: &str, request: SearchRequest) -> Result<Vec<SearchHit>>`
  - `pub fn validate_collection_name(name: &str) -> Result<()>`; `pub fn resolve_doc_id(input: &IngestInput, content_hash: &str) -> Result<String>`; `pub fn point_id(collection: &str, doc_id: &str, content_hash: &str, chunk_index: usize) -> uuid::Uuid`; `pub fn validate_search_request(req: &SearchRequest) -> Result<()>`.

- [ ] **Step 1: Write the failing tests**

Unit tests in `service.rs`:
1. `validates_collection_names`: accepts `a`, `docs-2026_01`; rejects empty, `-bad`, `bad name`, `__meta`, 65-char names, names with `/`.
2. `resolves_doc_id_precedence`: explicit wins; else `metadata.source` gives `doc-<16 hex>`; else content hash gives `doc-<16 hex>`.
3. `builds_filter_for_search`: `doc_ids + tags_any + source` produces `must` with a keyword match for source, keywords match for doc_ids, keywords match for tags; no filter returns `None`.
4. `payload_roundtrip_metadata`: payload built for a chunk parses back into `SearchHit` metadata.
5. `validates_search_limits`: empty query, `top_k=0`, and `top_k=51` are rejected; `top_k=1` and `top_k=50` are accepted.

Integration tests in `tests/store_service.rs` (all start with:
```rust
fn qdrant_url() -> Option<String> {
    match std::env::var("KB_TEST_QDRANT_URL") {
        Ok(v) if !v.is_empty() => Some(v),
        _ => { eprintln!("skipping: KB_TEST_QDRANT_URL not set"); None }
    }
}
```
and return early when `None`):

1. `lifecycle_create_ingest_search_delete`: unique collection name; create; ingest markdown text with tags; search a query; assert hits contain the text and metadata; re-ingest identical → `skipped=true`, `metadata_updated=false`; delete document → count 0; delete collection.
2. `reinject_repairs_missing_points_and_stale_tail`: ingest; delete one real point and insert a fake point with the same `doc_id+content_hash` and `chunk_index=999` (so the count still equals `chunk_count`); re-ingest same text → not skipped; final id set equals the expected set and the fake point is gone. Then insert a same-hash tail point (index 999) again without deleting anything; re-ingest → cleanup removes it and count equals chunk count.
3. `concurrent_ingests_of_same_doc_leave_one_complete_version`: same `doc_id`, two different texts, `tokio::join!`; after both, count points and distinct `content_hash` values; exactly one hash remains and its point count equals that version's chunk count.
4. `reinject_after_partial_write_converges`: ingest version A; manually upsert only the first point of version B with the correct id/payload; re-ingest version B; assert all B points exist, no A points.
5. `metadata_only_change_updates_payload_without_embedding`: mock embedder call count before/after; same text different tags → `metadata_updated=true`, `skipped=false`, calls unchanged; search filter by new tag finds it.
6. `delete_document_with_unicode_and_slash_ids`: doc_id `"手册/2026 版"`; ingest and delete; count 0.
7. `missing_collection_maps_to_not_found`: search/ingest/delete on `no-such-collection` → `Err(KbError::CollectionNotFound(_))`.
8. `dimension_mismatch_maps_to_409_error`: create collection with vector size 4 via store, mock embedder dim 8, ingest → `Err(KbError::DimensionMismatch(_))`.
9. `search_filter_limits_results`: two docs with different tags; `tags_any` returns only the matching doc; `doc_ids` narrows to one.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-core service::tests::validates_collection_names`
Expected: compile error / unimplemented.

- [ ] **Step 3: Implement `QdrantStore`**

- `Qdrant::from_url(&config.url).api_key(...)` only when `config.api_key` is non-empty, `.timeout(Duration::from_secs(...)).skip_compatibility_check().build()`.
- `wait_ready`: loop `list_collections()` every 500 ms until success or retry expires; on expiry return `KbError::Qdrant` with URL and last error.
- Map Qdrant errors: message contains "doesn't exist" / "not found" → `CollectionNotFound`; contains "already exists" → `CollectionExists`; contains "dimension" → `DimensionMismatch`; else `Qdrant`.
- `create_collection`: `CreateCollectionBuilder::new(name).vectors_config(VectorParamsBuilder::new(dim, Distance::Cosine))`.
- `collection_info`: extract vector size from `config.params.vectors_config.config` (`Config::Params(p) => p.size`); points count `points_count.unwrap_or(0)`; missing collection → `CollectionNotFound`.
- `upsert`: `UpsertPointsBuilder::new(collection, points).wait(true)`.
- `delete_by_filter`: `DeletePointsBuilder::new(collection).points(filter).wait(true)`.
- `count`: `CountPointsBuilder::new(collection).filter(filter).exact(true)`.
- `scroll_payloads`: `ScrollPointsBuilder::new(collection).filter(filter).limit(limit).with_payload(true)`; convert each `RetrievedPoint.payload` (`HashMap<String, Value>`) into a `serde_json::Map`.
- `scroll_ids`: same scroll with `limit(limit)` and `with_payload(false)`; extract `Uuid` point ids (conversion error → `KbError::Qdrant`).
- `set_metadata_payload`: build `HashMap<String, Value>` and `SetPayloadPointsBuilder::new(collection).payload(map).points_selector(filter).wait(true)`.
- `search`: `QueryPointsBuilder::new(collection).query(vector).limit(limit).with_payload(true)`; apply threshold/filter when present.

- [ ] **Step 4: Implement `KnowledgeBaseService`**

Helpers:
- `StripedLocks { stripes: Vec<tokio::sync::Mutex<()>> }` with 64 entries, `new()`, and `async fn lock(&self, key: &str) -> tokio::sync::MutexGuard<'_, ()>` using `DefaultHasher`.
- `content_hash`: SHA-256 over normalized text (parser output), hex lowercase.
- `resolve_doc_id`: explicit, else `doc-{first16(sha256(source))}`, else `doc-{first16(content_hash)}`; validate 1–128 chars, non-blank, no control chars.
- `point_id(collection, doc_id, hash, chunk_index) -> Uuid` using `Uuid::new_v5(&Uuid::NAMESPACE_OID, "{collection}:{doc_id}:{hash}:{chunk_index}")`; `point_ids(...) -> Vec<Uuid>` maps over chunk indices.
- `payload_for(...)` exactly per spec 7.4; `embedding_model` from provider.
- `metadata_matches(payload, meta) -> bool` treating missing and null as `None`, tags as ordered lists; the service requires every payload returned by `scroll_payloads` to match before treating metadata as unchanged.
- `metadata_payload(meta) -> Map<String, Value>` with `source`, `title`, `tags`, `extra`, using `Value::Null` for absent source/title.

Methods:
- `health`: try `store.list_collections()`; qdrant `"ok"`/`"unavailable"`, overall `"ok"`/`"degraded"`, embedding info from provider.
- `create_collection`: validate name; if exists → `CollectionExists`; create with provider dimension; return `CollectionInfo { points_count: 0, vector_size: dim }`.
- `list_collections`: delegate.
- `delete_collection`: if missing → `CollectionNotFound`; delete.
- `ingest_document`: per spec 7.3 and 7.6:
  1. validate collection name; ensure collection exists (404 otherwise).
  2. parse text; chunk; hash; resolve doc id; compute ids.
  3. acquire stripe lock for `collection + doc_id`.
  4. count filter `doc_id == X AND content_hash == H`. If count equals chunk count, also `scroll_ids` the same filter (limit `chunk_count + 1`) and compare the id set with the expected id set. Only when both count and set match: fetch all payloads with `scroll_payloads(filter, chunk_count as u32 + 1)` and require every payload's metadata to match; if any differ call `set_metadata_payload` once (`metadata_updated=true`); then cleanup; return `skipped = !metadata_updated`. If the id set differs, fall through to re-embedding.
  5. else embed each chunk's embedding text (`heading_path` + `"\n"` + text when non-empty), upsert in batches of `upsert_batch_size`, then cleanup `doc_id == X AND must_not has_id(ids)`, return counts.
- `delete_document`: validate doc id; ensure collection appears via `collection_exists` (404 otherwise); lock stripe; count then delete filter; return count.
- `search`: call `validate_search_request`; ensure collection exists (404); embed query; build filter; call store; map `ScoredPoint.payload` into `SearchHit` with `SearchHitMetadata { source, title, tags, heading_path, extra }` (fallback empty values only when fields are missing; do not panic).
- Qdrant dimension errors raised during upsert/search map to `DimensionMismatch` via the store mapping.

`scripts/start-qdrant.ps1`: if port 6334 is listening, print and exit 0; otherwise download `https://github.com/qdrant/qdrant/releases/download/v1.20.2/qdrant-x86_64-pc-windows-msvc.zip` into `.tools/qdrant.zip`, expand to `.tools/qdrant`, and `Start-Process -FilePath .tools/qdrant/qdrant.exe -WorkingDirectory .tools/qdrant`; wait until port 6334 accepts TCP for up to 30 s.

- [ ] **Step 5: Run unit tests, then integration tests with Qdrant**

Run: `cargo test -p kb-core service::tests`
Expected: PASS.

Start Qdrant: `powershell -ExecutionPolicy Bypass -File scripts/start-qdrant.ps1`
Then: `$env:KB_TEST_QDRANT_URL="http://127.0.0.1:6334"; cargo test -p kb-core --test store_service`
Expected: all 9 integration tests PASS (not skipped).

- [ ] **Step 6: Commit**

```bash
git add crates/kb-core/src/store.rs crates/kb-core/src/service.rs crates/kb-core/tests/store_service.rs scripts/start-qdrant.ps1
git commit -m "feat: add qdrant store and knowledge base service" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 7: HTTP server

**Files:**
- Modify: `crates/kb-server/src/main.rs`
- Create: `crates/kb-server/src/state.rs`, `crates/kb-server/src/error.rs`
- Create: `crates/kb-server/src/api/mod.rs`, `routes.rs`, `handlers.rs`
- Create: `crates/kb-server/tests/api.rs`

**Interfaces:**
- Consumes: `KnowledgeBaseService`, `QdrantStore`, `EmbeddingProvider`, `ApiEmbedding`, `LocalEmbedding` (feature-gated), configs, models.
- Produces:
  - `pub struct AppState { pub service: Arc<KnowledgeBaseService>, pub api_key: Option<String>, pub max_body_bytes: usize, pub request_timeout: Duration, pub embedding: EmbeddingInfo }`
  - `pub fn build_router(state: AppState) -> axum::Router`
  - `pub fn with_common_layers(router: axum::Router, max_body_bytes: usize, timeout: Duration) -> axum::Router` (used by `build_router` and by the timeout/body-limit tests)
  - `pub struct ApiError(pub KbError)` implementing `IntoResponse`
  - `pub async fn build_embedder(cfg: &EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>>`

- [ ] **Step 1: Write the failing tests**

`crates/kb-server/tests/api.rs`:

1. `auth_rejects_missing_or_wrong_token`: state with `api_key: Some("secret")`, mock embedder, service pointing at an unused Qdrant URL; `POST /v1/collections` with no token → 401 with `error.code == "UNAUTHORIZED"`; wrong token → 401.
2. `auth_accepts_valid_token_and_validation_runs`: same state, `Authorization: Bearer secret`, body `{"name":"bad name!"}` → 400 `VALIDATION_ERROR` (no Qdrant call needed).
3. `malformed_json_returns_validation_envelope`: valid auth, body `{` → 400 `VALIDATION_ERROR`.
4. `health_is_unauthenticated_and_reports_degraded`: build the store with `url = "http://127.0.0.1:1"` and `timeout_secs = 1`; `GET /health` without token → 503, JSON `{"status":"degraded","qdrant":"unavailable",...}`.
5. `timeout_returns_408_envelope`: use `with_common_layers` around a router with a slow test handler (sleep > timeout) → 408 and `error.code == "REQUEST_TIMEOUT"`.
6. `body_limit_returns_413_envelope`: use `with_common_layers` with a small `max_body_bytes`, send a larger body → 413 and `error.code == "PAYLOAD_TOO_LARGE"`.
7. `unknown_format_returns_415`: valid auth and valid collection name, body `{"text":"hi","format":"pdf"}` → 415 `UNSUPPORTED_FORMAT` (format parsing happens before any Qdrant call).
8. `full_lifecycle_over_http` (Qdrant-gated): create collection; ingest markdown with tags; search and find text; re-ingest → `skipped=true`; upload a text file via multipart; delete document; list collections; delete collection.
9. `unicode_doc_id_delete_over_http` (Qdrant-gated): ingest with `doc_id="手册/2026 版"`, then `DELETE /v1/collections/{name}/documents?doc_id=%E6%89%8B%E5%86%8C%2F2026%20%E7%89%88` → 200 `deleted_points > 0`.
10. `api_provider_end_to_end` (Qdrant-gated): start a wiremock server returning an 8-dim vector for any input; build `ApiEmbedding` with `dimensions=8`, a store, and a service; create collection, ingest, search → non-empty result. This covers the `provider=api` wiring without real credentials.

- [ ] **Step 2: Run tests to verify they fail**

Run: `cargo test -p kb-server auth_rejects_missing_or_wrong_token`
Expected: compile error / no router.

- [ ] **Step 3: Implement the HTTP layer**

- `ApiError`: map `KbError` variants per the spec table (`Config/Internal/Io -> 500 INTERNAL`, `Validation/Parse -> 400 VALIDATION_ERROR`, `InvalidEncoding -> 400 INVALID_ENCODING`, `UnsupportedFormat -> 415 UNSUPPORTED_FORMAT`, `Unauthorized -> 401`, `CollectionNotFound -> 404`, `CollectionExists -> 409`, `DimensionMismatch -> 409 EMBEDDING_DIM_MISMATCH`, `Embedding -> 502 EMBEDDING_ERROR`, `Qdrant -> 502 QDRANT_ERROR`). Serialize `{"error":{"code","message"}}`.
- Router: build `Router::new().route("/health", get(health)).nest("/v1", v1_router).with_state(state)`, then wrap it with `with_common_layers(router, max_body_bytes, timeout)`. That helper applies, from inner to outer: `DefaultBodyLimit::max`, `TimeoutLayer`, `map_response(rewrite_status_envelopes)`, `TraceLayer::new_for_http`.
  v1 routes: `GET/POST /collections`, `GET/DELETE /collections/{name}`, `POST/DELETE /collections/{name}/documents`, `POST /collections/{name}/documents/upload`, `POST /collections/{name}/search`.
  v1 router applies auth middleware via `middleware::from_fn_with_state`; auth compares `Authorization: Bearer <api_key>` when configured.
- `rewrite_status_envelopes`: if response status is 408 → JSON `REQUEST_TIMEOUT`; 413 → JSON `PAYLOAD_TOO_LARGE`; otherwise pass through.
- Handlers:
  - `health`: `service.health()`; 200 when qdrant ok, 503 otherwise.
  - `list_collections`: `{"collections":[...]}`; `create_collection` (201, `CollectionInfo`); `get_collection` (`CollectionInfo`); `delete_collection` (204).
  - `ingest_document`: `Json<IngestDocumentRequest>`; `format` defaults to `Text`, otherwise `DocumentFormat::parse` (unknown → 415); convert to `IngestInput`; return `IngestResult`.
  - `upload_document`: `Multipart`; read `file`, `doc_id`, `source`, `title`, `tags` (repeatable or comma-separated), `format`, `encoding`; `decode_bytes`; explicit `format` goes through `DocumentFormat::parse` (unknown → 415), otherwise infer from filename; call ingest; return `IngestResult`.
  - `delete_document`: `Query<DeleteDocumentQuery { doc_id: String }>` (the route has no `{doc_id}` segment); return `{"doc_id":..., "deleted_points": n}`.
  - `search`: validate via service; return `{"results":[...]}`.
  - JSON/query/multipart rejections are mapped to `VALIDATION_ERROR` (use `Result<Json<T>, JsonRejection>`, `Result<Query<T>, QueryRejection>`, `Result<Multipart, MultipartRejection>`).
- `build_embedder`: API → `ApiEmbedding::new(...).await`; local under `#[cfg(feature = "local-embedding")]` → `tokio::task::spawn_blocking(move || LocalEmbedding::new(...)).await`; without the feature → `KbError::Config("local-embedding feature is not enabled")`.
- `main`: parse `--config <path>` (also honor `KB_CONFIG`), init `tracing_subscriber` with `RUST_LOG` default `info,kb_core=debug`, build store, `wait_ready`, build embedder, build state/router, bind `host:port`, log safe startup info, serve with graceful shutdown on Ctrl-C and (unix) SIGTERM. Mask API keys in logs.

- [ ] **Step 4: Run tests to verify they pass**

Run: `cargo test -p kb-server` without `KB_TEST_QDRANT_URL`
Expected: tests 1–7 PASS; 8–10 print skip.
Then with Qdrant running and `KB_TEST_QDRANT_URL=http://127.0.0.1:6334`, run again.
Expected: all PASS.

- [ ] **Step 5: Commit**

```bash
git add crates/kb-server
git commit -m "feat: expose REST knowledge base API" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 8: Docker, compose, example environment, README

**Files:**
- Create: `Dockerfile`, `docker-compose.yml`, `.env.example`, `README.md`

**Interfaces:**
- Consumes: the final HTTP API and config keys from Tasks 1–7.
- Produces: runnable deployment files and the complete Chinese README.

- [ ] **Step 1: Write the deployment files**

`Dockerfile`: multi-stage. Builder `rust:1.100-bookworm`; copy manifests + `crates`; `cargo build --release -p kb-server` (default features include local embedding). Runtime `debian:bookworm-slim` with `ca-certificates` and `libstdc++6`; copy `kb-server`; copy `config/config.example.toml` to `/app/config/config.toml`; `EXPOSE 8080`; `CMD ["kb-server"]`.

`docker-compose.yml`: `qdrant` service `qdrant/qdrant:v1.20.2` with ports 6333/6334 and `./data/qdrant:/qdrant/storage`; `kb-server` build `.`, port 8080, env `KB_QDRANT__URL=http://qdrant:6334` and `KB_EMBEDDING__LOCAL__CACHE_DIR=/data/models`, volume `./data/models:/data/models`, `depends_on: [qdrant]`, `restart: unless-stopped`.

`.env.example`: documented optional overrides (`KB_EMBEDDING__PROVIDER`, API keys, HF endpoint), no real secrets.

- [ ] **Step 2: Write the README**

Chinese README with these sections and content:

1. 项目简介 + 功能特性 + 架构 Mermaid 图。
2. 目录结构。
3. 快速开始 A（Windows 无 Docker）：`scripts/start-qdrant.ps1` → 配置 → `cargo run -p kb-server`；快速开始 B：`docker compose up --build`。
4. 配置说明：完整键名/默认值/环境变量映射表（与 Task 1 一致）。
5. Embedding 配置：本地模型别名/维度/前缀表、HF 镜像、API 兼容端点示例（OpenAI、通义兼容模式、SiliconFlow、Ollama、vLLM），并说明换模型要重建集合。
6. API 文档：每个端点的 curl 示例（Windows 用 `curl.exe`；含建集合、文本注入、文件上传、检索、删除文档、删除集合、健康检查）和错误码表。
7. 数据模型：payload 字段、切块规则、幂等/对账、单实例写保证。
8. agent 接入示例：Python（`httpx`）与 TypeScript（`fetch`）各一段完整检索/注入代码。
9. 测试：`cargo test --workspace`、启动 Qdrant 后设置 `KB_TEST_QDRANT_URL` 的集成测试、`#[ignore]` 本地模型测试。
10. 扩展指南：新增解析器、新增 embedding provider 的步骤。
11. FAQ/排障：6333 vs 6334、模型下载失败、MSVC/ORT 构建、维度不匹配、GBK 上传、Windows 二进制启动、Docker 部署。
12. 已知限制与 Roadmap（单实例、无 hybrid/rerank/PDF、异步任务等）。

All commands and JSON examples must match the implemented API exactly.

- [ ] **Step 3: Verify syntax and consistency**

Run: `cargo fmt --check && cargo clippy --all-targets -- -D warnings`
Run: `docker compose config` if Docker is available; on this machine it is not, so record that only file review was possible.
Check README links and referenced paths exist.

- [ ] **Step 4: Commit**

```bash
git add Dockerfile docker-compose.yml .env.example README.md
git commit -m "docs: add deployment files and complete README" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```

---

### Task 9: End-to-end verification

**Files:**
- Modify: any files needed for fixes.
- Verify: `README.md` commands, `.env.example`, `config/config.example.toml`.

**Interfaces:**
- Consumes: everything.
- Produces: verified working service; evidence in the final report.

- [ ] **Step 1: Static checks**

Run: `cargo fmt --check`
Run: `cargo clippy --all-targets -- -D warnings`
Run: `cargo build --no-default-features -p kb-server` (API-only build compiles)
Run: `cargo test --workspace` (unit tests pass)
Expected: all clean.

- [ ] **Step 2: Integration tests with real Qdrant**

Ensure Qdrant is running (`scripts/start-qdrant.ps1`). Then:

```powershell
$env:KB_TEST_QDRANT_URL="http://127.0.0.1:6334"
cargo test --workspace
```

Expected: all tests PASS, no skips for Qdrant-gated tests.

- [ ] **Step 3: Real server smoke test**

Create a temporary config pointing at `http://127.0.0.1:6334`, local embedding `bge-small-zh-v1.5`, `cache_dir=.tools/models`. Run `cargo run -p kb-server` (first run downloads the model; if Hugging Face is unreachable, set `HF_ENDPOINT=https://hf-mirror.com` first and retry). Then execute the README curl flow exactly:

1. `GET /health` → 200, qdrant ok, dimension 512.
2. `POST /v1/collections` `{"name":"readme-demo"}` → 201.
3. `POST /v1/collections/readme-demo/documents` with a Chinese Markdown document and tags.
4. Same request again → `skipped=true`.
5. Change only tags → `metadata_updated=true`.
6. `POST /v1/collections/readme-demo/search` → non-empty results containing the document text.
7. `DELETE /v1/collections/readme-demo/documents?doc_id=...` → deleted_points > 0.
8. `DELETE /v1/collections/readme-demo` → 204.
9. `GET /health` with the server stopped/restarted → data survives restart (create + ingest again before restart).

- [ ] **Step 4: API-embedding smoke test without credentials**

Run the `api_provider_end_to_end` test from Task 7 with Qdrant running:
`cargo test -p kb-server --test api api_provider_end_to_end -- --nocapture`
Expected: PASS, no real credentials used.

- [ ] **Step 5: Final review and commit**

Run `git status` and ensure only intended files changed. Fix any discrepancy between README and behaviour, update the design doc only if implementation had to deviate (note it in the commit message). Commit:

```bash
git add -A
git commit -m "test: verify end-to-end knowledge base flow" -m "Co-Authored-By: Claude Code <noreply@anthropic.com>"
```