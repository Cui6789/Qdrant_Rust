# Qdrant 知识库服务设计文档

- 日期：2026-11-25
- 状态：已批准（2026-11-25）
- 作者：知识库架构（Claude Code + 用户）
- 目标读者：负责实现本模块的工程师

---

## 1. 背景与目标

公司正在建设一个 agent 项目，需要一个**独立运行的知识库服务**：agent（无论用什么语言实现）通过 HTTP 调用它，完成知识文档的注入、向量化存储与语义检索。

核心诉求：

1. 向量存储使用 **Qdrant**，客户端用 **Rust** 实现。
2. 嵌入模型必须可插拔，至少提供两类实现：
   - **本地嵌入式模型**：进程内运行（ONNX），无需额外服务，适合私有化/离线/低成本场景；
   - **API 嵌入模型**：调用 OpenAI 兼容的 `/v1/embeddings` 接口，可对接 OpenAI、通义、SiliconFlow、Ollama、vLLM 等。
3. 知识文档注入要开箱即用：解析 → 切块 → 向量化 → 写入，调用方只需提交文本或文件。
4. 交付物必须模块化：核心逻辑与 HTTP 外壳分离，外部调用简单，README 完整（快速开始、配置、API、扩展点、排障）。
5. 本模块是 agent 项目的**基础设施组件**，不是一次性脚本：可测试、可部署、可扩展。

为什么是独立服务：用户明确选择「独立服务」形态，让非 Rust 的 agent 也能零成本接入；知识库作为共享基础设施，独立部署、独立升级，避免与每个 agent 进程耦合。

---

## 2. 范围

### 2.1 Goals

- REST/JSON 同步 API（axum），curl 可直接调用。
- 两个 embedding 实现，通过配置切换，统一 trait 抽象。
- Qdrant 集合管理（创建/列出/删除）与按知识库隔离。
- 文档注入：纯文本 / Markdown / HTML / JSON；文件上传；同步返回结果。
- 检索：向量相似度 + top_k + 阈值 + 简单元数据过滤；返回原文、分数、元数据。
- 幂等：同一文档重复注入不重复写入；重注入自动清理旧分块。
- 按文档删除（`doc_id`）；健康检查；可选 Bearer 鉴权。
- 完整测试（单元 + HTTP 集成）、Docker Compose、Dockerfile、完整中文 README。

### 2.2 Non-goals（v1 明确不做）

- PDF / Word 解析（留解析器扩展点）。
- 稀疏向量 / hybrid 检索、rerank、MMR。
- 异步任务队列（大文档注入同步完成；核心层预留 job 层扩展空间）。
- 多租户细粒度权限（仅可选全局 Bearer Token）。
- 每个集合使用不同 embedding 模型（一个服务实例一个 provider）。
- 多副本并发写同一文档（v1 单实例 + 进程内条带锁，见 7.6）。
- 上传文件字符集自动探测（默认 UTF-8，GBK/GB18030 需显式指定 encoding）。
- 应用层限流（由网关/部署层负责）。
- 管理 UI、Prometheus 指标、OpenAPI/Swagger 页面。
- 文档源增量同步（爬虫、定时同步等）。

### 2.3 Anti-goals（即使能跑也算失败）

- 强制依赖云端 embedding API，导致无法离线使用。
- 调用方必须理解 Qdrant 概念（collection/filter/point）才能注入或检索。
- 核心逻辑与 HTTP 框架耦合，未来换 gRPC 或直接做库用时需要重写。
- README 只有「能编译」级别的说明，缺少可照抄的调用示例与排障说明。

---

## 3. 方案选择

### 3.1 服务形态

| 方案 | 说明 | 取舍 |
|------|------|------|
| **REST 同步（选中）** | axum + JSON；ingest/search/delete 同步返回 | curl 可调、任何语言零门槛；超大文档会占住请求，靠客户端/服务端超时兜底 |
| gRPC（tonic） | protobuf，强类型、高吞吐 | 调用方要生成 stub，临时调试不便 |
| REST + 异步任务 | ingest 返回 job_id，后台轮询 | 适合大批量/慢 API，但引入队列与状态管理，v1 过度设计 |

选择 REST 同步：agent 场景下文档注入不是高频操作，同步 + 批量 upsert 足够；核心层按用例分层，将来加异步 job 层不推翻核心。

### 3.2 本地嵌入实现

| 方案 | 取舍 |
|------|------|
| **fastembed（选中）** | 纯 Rust 封装 ONNX Runtime，进程内推理，无需外部服务；首次运行自动下载模型；CPU 足够应付知识库注入 |
| 调用 Ollama/llama.cpp HTTP | 部署简单但多一个外部依赖，且与「API 模型」接口重复 |
| candle / 手写 transformers | 控制力强、实现成本与模型适配成本高，不值得 |

### 3.3 Qdrant 接入

- `qdrant-client` **1.20.0**：仅远程 gRPC（默认端口 6334；REST 6333 仅用于快照下载），**没有进程内本地模式**（不存在 `Qdrant::from_path`；`qdrant-edge` 是 shard 级 API 且需 C++ 工具链，不采用）。
- 本地开发/测试使用 Qdrant 官方 Windows 二进制（v1.20.2，`qdrant-x86_64-pc-windows-msvc.zip`，约 32 MB）；Docker Compose 作为部署方式。
- 客户端构造：`Qdrant::from_url(url).api_key(...).timeout(...).build()`。客户端默认会做兼容性检查，服务器不可达时只打印警告不失败，因此服务启动阶段主动重试 `list_collections`（`qdrant.startup_retry_secs`，默认 30 秒），超时后带清晰原因退出。
- 集合距离统一使用 **Cosine**。
- 写操作使用 `wait(true)`，保证注入接口返回时数据已可见。

---

## 4. 架构总览

```
┌──────────────────────────────┐
│  agent / 任意 HTTP 客户端      │
└──────────────┬───────────────┘
               │ REST/JSON (/v1)
┌──────────────▼───────────────┐
│ kb-server (axum)             │  鉴权 / 超时 / 请求校验 / 错误映射
│  handlers → KnowledgeBaseService
└──────────────┬───────────────┘
┌──────────────▼───────────────┐
│ kb-core                      │
│   ingest: parser → chunker   │
│   embedding: trait + 2 impls │
│   store: QdrantStore         │
└───────┬──────────────┬───────┘
        │              │
┌───────▼──────┐ ┌─────▼─────────────┐
│ fastembed    │ │ OpenAI 兼容 API    │
│ (ONNX 本地)   │ │ /v1/embeddings     │
└──────────────┘ └───────────────────┘
┌──────────────────────────────┐
│ Qdrant (gRPC 6334)           │
└──────────────────────────────┘
```

- `kb-core` 不依赖 axum；`kb-server` 依赖 `kb-core`。依赖方向单向。
- 数据流（注入）：HTTP 请求 → 解析为纯文本 → 切块 → 批量 embed → 批量 upsert → 清理旧版本分块 → 返回统计。
- 数据流（检索）：query → embed_query → Qdrant search（payload 回传）→ 结果映射。

---

## 5. 目录结构（交付物）

```
D:\RAG\
├── Cargo.toml                       # workspace
├── crates/
│   ├── kb-core/
│   │   └── src/
│   │       ├── lib.rs
│   │       ├── config.rs            # 配置结构 + 加载（TOML + 环境变量）
│   │       ├── error.rs             # KbError
│   │       ├── models.rs            # DTO：DocumentInput/SearchRequest/Payload 等
│   │       ├── embedding/
│   │       │   ├── mod.rs           # EmbeddingProvider trait + factory
│   │       │   ├── local.rs         # fastembed（feature: local-embedding）
│   │       │   └── api.rs           # OpenAI 兼容
│   │       ├── ingest/
│   │       │   ├── mod.rs
│   │       │   ├── parser.rs        # text/markdown/html/json
│   │       │   └── chunker.rs       # 标题感知 + 长度切块
│   │       ├── store.rs             # QdrantStore 封装
│   │       └── service.rs           # KnowledgeBaseService（用例编排）
│   └── kb-server/
│       ├── src/
│       │   ├── main.rs              # 装配 + 启动 + 优雅退出
│       │   ├── state.rs
│       │   ├── error.rs             # KbError → HTTP 响应
│       │   └── api/
│       │       ├── mod.rs
│       │       ├── routes.rs
│       │       └── handlers.rs
│       └── tests/api.rs             # HTTP 集成测试
├── config/config.example.toml
├── docker-compose.yml               # qdrant + kb-server
├── Dockerfile                       # 多阶段构建
├── .env.example
├── .gitignore
└── README.md
```

---

## 6. Embedding 抽象

### 6.1 Trait

```rust
#[async_trait::async_trait]
pub trait EmbeddingProvider: Send + Sync {
    fn model_id(&self) -> &str;
    fn dimension(&self) -> usize;
    async fn embed_documents(&self, texts: &[String]) -> Result<Vec<Vec<f32>>, KbError>;
    async fn embed_query(&self, text: &str) -> Result<Vec<f32>, KbError>;
}
```

- 工厂 `create_embedder(&EmbeddingConfig) -> Result<Arc<dyn EmbeddingProvider>>`。
- 服务启动时初始化（含维度探测），配置错误/模型不可用直接启动失败，附带可读原因。

### 6.2 本地实现（fastembed）

- 版本：`fastembed` **7.8.0**。初始化：`TextEmbedding::try_new(TextInitOptions::new(EmbeddingModel::…).with_cache_dir(…).with_show_download_progress(…).with_max_length(…))`。
- 调用：`embed(&mut self, texts, batch_size) -> Vec<Vec<f32>>`，同步、已做 L2 归一化；因此模型放在 `tokio::sync::Mutex` 后，embed 调用走 `spawn_blocking`，避免阻塞异步 runtime。
- 维度：`TextEmbedding::get_model_info(&model)?.dim`，无需加载模型。
- 模型别名（fastembed 7.8.0 实际支持）：
  - 中文：`bge-small-zh-v1.5`(512)、`bge-large-zh-v1.5`(1024)、`bge-m3`(1024)、`multilingual-e5-small/base/large`(384/768/1024)
  - 英文：`bge-small-en-v1.5`(384)、`all-minilm-l6-v2`(384)、`nomic-embed-text-v1.5`(768)
  - 未知名称 → `VALIDATION_ERROR`，报错中列出可用别名。
- 前缀：crate **不会自动添加** query/passage 前缀，由 provider 按模型补默认值，配置可覆盖：
  - `bge-small-zh-v1.5`：查询前缀 `为这个句子生成表示以用于检索相关文章：`，文档无前缀；
  - `multilingual-e5-*`：查询 `query: `，文档 `passage: `；
  - `nomic-embed-text-*`：查询 `search_query: `，文档 `search_document: `；
  - `bge-m3` / `bge-en-*`：默认空，可按模型卡配置。
- 首次下载由 hf-hub 完成，支持 `HF_ENDPOINT`（如 `https://hf-mirror.com`）、`HF_HOME`、`HF_TOKEN`；缓存在 `embedding.local.cache_dir`。
- 构建：`local-embedding` 为默认 feature；`ort` 2.0.0-rc.15 在构建期自动下载预编译 ONNX Runtime（Windows 静态链接，需要 MSVC + Windows SDK，已验证本机 VS 2022 可用）。fastembed 默认 `onig` 需要 C 编译器。API-only 部署可用 `--no-default-features` 构建。

### 6.3 API 实现（OpenAI 兼容）

- `reqwest` + rustls；请求 `POST {base_url}/embeddings`，`Authorization: Bearer {api_key}`（key 为空则不带）。
- 请求体：`{"model": ..., "input": [...]}`；`dimensions > 0` 时附带。
- 按 `embedding.batch_size` 分批；`max_concurrent_batches` 用 `Semaphore` 限流。
- 429 / 5xx / 超时按指数退避重试（尊重 `Retry-After`），重试次数 `max_retries`。
- 维度：配置 `dimensions > 0` 用之；否则启动时用一条探针文本请求一次并缓存。
- 兼容性由「OpenAI 兼容协议」保证，不针对厂商写特例；README 给 OpenAI/通义/SiliconFlow/Ollama/vLLM 的配置示例。

---

## 7. 文档注入管道

### 7.1 解析（parser）

| 格式 | 行为 |
|------|------|
| `text` | 统一换行（CRLF→LF）、trim |
| `markdown` | 保留原文；标题行用于后续标题感知切块 |
| `html` | `scraper` 提取可见文本；移除 script/style/noscript；h1-h6 转为 `#` 标题行，保留结构 |
| `json` | 递归收集字符串叶子值并以换行拼接；顶层为字符串数组或含 `text` 字段的对象数组时按顺序提取 |

上传文件按扩展名推断格式（`.md/.markdown`→markdown，`.html/.htm`→html，`.json`→json，其余→text），可用表单字段显式覆盖。

文本解码：默认 UTF-8（去掉 BOM）；上传时可用 `encoding=gbk|gb18030` 显式转码（`encoding_rs`）。解码失败返回 `INVALID_ENCODING`。v1 不做字符集自动探测，也不解析 HTML `<meta charset>`。

解析后为空白（或切块后为空）的文档返回 `VALIDATION_ERROR`，不创建集合数据。

### 7.2 切块（chunker）

1. 按 Markdown 标题切分 section，维护标题路径（如 `第二章 > 休假制度`）。
2. section 内按空行段落切分；超长段落按句末标点（。！？!?.\n）切分；仍超长则硬切。
3. 将碎片按 `chunk_size`（字符数，默认 800）打包；相邻块保留 `chunk_overlap`（默认 120）字符重叠；尾部过小（< `min_chunk_size`）并入前块。
4. 每块记录 `heading_path`；embedding 文本为 `heading_path + "\n" + 块文本`（有标题时），payload 里保存原始块文本。
5. 字符数按 Unicode 标量计算（对中文友好），不追求精确 token 数。

### 7.3 身份、去重与幂等

- 归一化文本的 SHA-256（hex）作为 `content_hash`。
- `doc_id` 规则：
  - 调用方显式提供则使用；
  - 否则若有 `metadata.source`，取 `doc-` + sha256(source) 前 16 位；
  - 否则取 `doc-` + sha256(content) 前 16 位。
  - 校验：1–128 字符，非空白，无控制字符；允许 Unicode（删除接口用 query 参数传递，不放进路径段）。
- point id：UUID v5（固定命名空间），name = `{collection}:{doc_id}:{content_hash}:{chunk_index}`，可确定性重算。
- **每次注入都做对账**（包括文本未变化的场景）：
  1. 切块后按 `doc_id + content_hash` **count** 实际 point 数；若等于本次块数，再滚动取出这些 point 的 id 集合，与本次期望的 point id 集合逐一对齐。
  2. 若数量与 id 集合都一致：不再调用 embedding。逐块比较元数据（source/title/tags/extra）：
     - 相同 → `skipped=true`；
     - 不同 → 用 `set_payload`（filter `doc_id + content_hash`）原地更新元数据，返回 `metadata_updated=true`（不重新 embedding）。
     两种情况都继续执行第 4 步清理。
  3. 若数量或 id 集合不一致（上次中途失败、历史脏点或切块参数变化）→ 重新 embedding 并 upsert（point id 确定性，重复写入安全）。
  4. **清理**：upsert 全部成功后，删除 filter 为 `doc_id == X AND must_not has_id(本次 point ids)` 的 point。这同时清掉旧内容哈希的残留、同哈希但编号超出本次块数的尾块，以及历史脏数据。
- 写入顺序保证：新 point 全部 upsert 成功后才清理旧 point；中途失败时旧数据仍在，重试即可自愈。清理失败返回 `QDRANT_ERROR`，重试会再次对账并清理。
- 删除文档：先 count 再 delete（filter `doc_id == X`），返回删除的 point 数；不存在时返回 0（幂等）。
- 所有写路径由 7.6 的条带锁串行化。

### 7.4 Payload Schema（Qdrant point payload）

```json
{
  "doc_id": "handbook",
  "chunk_index": 3,
  "chunk_count": 12,
  "text": "……块原文……",
  "content_hash": "sha256hex",
  "source": "handbook.md",
  "title": "员工手册",
  "tags": ["hr", "policy"],
  "heading_path": "第二章 > 休假制度",
  "extra": {"dept": "HR"},
  "embedding_model": "bge-small-zh-v1.5",
  "created_at": "2026-11-25T12:00:00Z"
}
```

`source/title/heading_path/extra` 可缺省。`extra` 为调用方任意 JSON 对象，仅存储与返回，不参与 v1 过滤。

### 7.5 集合模型

- 每个知识库一个 Qdrant collection；名称校验 `^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$`。
- 创建时向量维度 = provider.dimension，距离 Cosine。
- 不额外维护元数据存储：集合状态（points_count、vector_size）直接读 Qdrant。
- 维度不匹配由 Qdrant 拒绝，服务映射为 `EMBEDDING_DIM_MISMATCH`（409），提示重建集合。
- 已知限制：更换**同维度**的不同 embedding 模型无法自动检测，需按 README 的重建流程处理（payload 中保留 `embedding_model` 便于排查）。

### 7.6 并发与一致性

- v1 按**单实例**部署。服务内用固定 64 个条带锁（striped locks，`tokio::sync::Mutex`），按 hash(collection + doc_id) 选锁，串行化同一文档的 ingest/delete；不同文档可并行。
- 该保证只在单进程内有效；多副本同时写同一文档不在 v1 支持范围。README 明确「单写实例」部署要求；水平扩展需外部写协调（分布式锁/单写路由），列为后续工作。
- 请求超时（`server.request_timeout_secs`）会取消 handler，可能发生在 upsert 部分完成后；由于写入顺序是「先新后清」且每次注入对账，客户端重试即可收敛到完整状态。
- 同一文档的不同内容并发注入在单实例下被串行化，不会互相删除；多副本下没有此保证。

---

## 8. 检索

请求：

```json
{
  "query": "年假怎么申请",
  "top_k": 5,
  "score_threshold": 0.2,
  "filter": {
    "doc_ids": ["handbook"],
    "tags_any": ["hr"],
    "source": "handbook.md"
  }
}
```

- `top_k` 默认 5，范围 1–50；`score_threshold` 可选。
- 过滤语义：不同字段之间 AND；`tags_any` 内部 OR；`doc_ids` 内部 OR。
- 返回按分数降序：

```json
{
  "results": [{
    "score": 0.83,
    "doc_id": "handbook",
    "chunk_index": 3,
    "text": "……",
    "metadata": {
      "source": "handbook.md",
      "title": "员工手册",
      "tags": ["hr"],
      "heading_path": "第二章 > 休假制度",
      "extra": {}
    }
  }]
}
```

---

## 9. HTTP API

统一前缀 `/v1`，JSON 请求/响应；`/health` 不需要鉴权。

| 方法 | 路径 | 说明 |
|------|------|------|
| GET | `/health` | Qdrant 连通性 + embedding 配置/维度 |
| GET | `/v1/collections` | 集合列表（名称、points_count、vector_size） |
| POST | `/v1/collections` | 创建集合 `{"name": "..."}`，201 |
| GET | `/v1/collections/{name}` | 集合详情 |
| DELETE | `/v1/collections/{name}` | 删除集合，204 |
| POST | `/v1/collections/{name}/documents` | 注入文本 |
| POST | `/v1/collections/{name}/documents/upload` | 上传文件（multipart） |
| DELETE | `/v1/collections/{name}/documents?doc_id=...` | 按文档删除（query 参数，支持任意 Unicode id） |
| POST | `/v1/collections/{name}/search` | 检索 |

注入请求：

```json
{
  "doc_id": "handbook",
  "text": "# 员工手册\n……",
  "format": "markdown",
  "metadata": {
    "source": "handbook.md",
    "title": "员工手册",
    "tags": ["hr"],
    "extra": {"dept": "HR"}
  }
}
```

注入响应：

```json
{
  "doc_id": "handbook",
  "chunks": 12,
  "skipped": false,
  "metadata_updated": false,
  "content_hash": "…",
  "embedding_model": "bge-small-zh-v1.5"
}
```

- `skipped=true`：文本与元数据都未变化，未调用 embedding。
- `metadata_updated=true`：文本未变、仅元数据变化，已原地更新 payload，未调用 embedding。
- 两者都为 false：执行了（或复用了幂等写入的）完整注入。

上传接口 multipart 字段：`file`（必填）、`doc_id`、`source`、`title`、`tags`（可重复或逗号分隔）、`format`、`encoding`（`utf-8` 默认 / `gbk` / `gb18030`）。

错误响应统一：

```json
{"error": {"code": "COLLECTION_NOT_FOUND", "message": "collection 'docs' does not exist"}}
```

错误码与 HTTP 状态：

| code | HTTP | 场景 |
|------|------|------|
| `VALIDATION_ERROR` | 400 | 参数/格式/集合名非法 |
| `INVALID_ENCODING` | 400 | 文件编码不支持或解码失败 |
| `UNAUTHORIZED` | 401 | Bearer token 缺失或错误 |
| `REQUEST_TIMEOUT` | 408 | 请求处理超过 `server.request_timeout_secs` |
| `COLLECTION_NOT_FOUND` | 404 | 集合不存在 |
| `COLLECTION_EXISTS` | 409 | 重复创建 |
| `EMBEDDING_DIM_MISMATCH` | 409 | 集合维度与当前模型不符 |
| `PAYLOAD_TOO_LARGE` | 413 | 请求体/上传文件超限 |
| `UNSUPPORTED_FORMAT` | 415 | 未知格式 |
| `EMBEDDING_ERROR` | 502 | 本地/API embedding 失败（重试后） |
| `QDRANT_ERROR` | 502 | Qdrant 调用失败 |
| `INTERNAL` | 500 | 其他 |

超时由服务端中间件统一返回 408；handler 被取消时可能留下部分新 point，按 7.3 的对账逻辑重试即可修复。Qdrant 的维度错误会映射为 `EMBEDDING_DIM_MISMATCH`。

---

## 10. 配置

加载顺序：内置默认值 → `config/config.toml`（存在则读）→ 环境变量（前缀 `KB_`，分隔符 `__`）。配置文件路径可被 `KB_CONFIG` 或 `--config` 覆盖。

```toml
[server]
host = "0.0.0.0"
port = 8080
api_key = ""                  # 非空则要求 Authorization: Bearer <key>
max_body_mb = 32
request_timeout_secs = 300

[qdrant]
url = "http://localhost:6334" # gRPC 端口（REST 6333 不用于 CRUD）
api_key = ""
timeout_secs = 30
startup_retry_secs = 30       # 启动时等待 Qdrant 就绪

[embedding]
provider = "local"            # local | api
batch_size = 32
max_concurrent_batches = 4

[embedding.local]
model = "bge-small-zh-v1.5"
cache_dir = "./data/models"
show_download_progress = true
max_length = 512
warmup = true
query_prefix = ""             # 留空使用模型默认前缀
document_prefix = ""

[embedding.api]
base_url = "https://api.openai.com/v1"
api_key = ""
model = "text-embedding-3-small"
dimensions = 0                # 0 = 探针获取
timeout_secs = 60
max_retries = 3

[ingest]
chunk_size = 800              # 字符数
chunk_overlap = 120
min_chunk_size = 30
upsert_batch_size = 64
```

常用环境变量示例：

```bash
KB_SERVER__PORT=9090
KB_QDRANT__URL=http://qdrant:6334
KB_EMBEDDING__PROVIDER=api
KB_EMBEDDING__API__BASE_URL=https://api.siliconflow.cn/v1
KB_EMBEDDING__API__API_KEY=sk-xxx
KB_EMBEDDING__API__MODEL=BAAI/bge-m3
```

实现选型：`figment`（Toml + Env provider）或等价的轻量方案；README 给出完整配置表。

---

## 11. 错误处理与可观测性

- `kb-core` 统一 `KbError`（`thiserror`），携带可读消息与来源；`kb-server` 映射为错误码 + HTTP 状态。
- 所有请求记录 `tracing` span（method、path、status、耗时）；`RUST_LOG` 控制级别，默认 `info,kb_core=debug`。
- 启动日志打印：监听地址、Qdrant 地址、embedding provider/model/维度（不打印 api_key）。
- `/health`：Qdrant `list_collections`（短超时）+ provider 就绪信息；Qdrant 不可达返回 503。
- 优雅退出：监听 Ctrl-C / SIGTERM，停止接受新请求。

---

## 12. 测试策略

- **单元测试（kb-core）**
  - 切块：中文长文、标题路径、重叠、尾块合并、空文本。
  - 解析：HTML 去 script/style、标题转换；JSON 扁平化；格式推断。
  - 身份：doc_id 规则、content_hash、point id 确定性。
  - 配置：默认值、TOML、环境变量覆盖。
  - 过滤条件 → Qdrant filter 转换。
- **模拟 embedding**：测试用确定性向量（哈希派生、归一化、固定 64 维），不依赖网络/模型。
- **集成测试**
  - Qdrant：`qdrant-client` 无进程内模式，测试通过 `KB_TEST_QDRANT_URL` 指向本地 Qdrant 运行；未设置时相关集成测试跳过。开发机流程：解压并启动 Qdrant Windows 二进制（gRPC 6334）→ 设置该环境变量 → `cargo test`。
  - HTTP：axum `oneshot` + 模拟 embedding，覆盖：建集合→注入→检索→删除→删集合、幂等跳过、维度不匹配、鉴权、错误码。不依赖 Qdrant 的用例（参数校验、鉴权）可直接跑；数据路径用例跟随 `KB_TEST_QDRANT_URL` gating。
  - 一致性与恢复：同一 `doc_id` 两个不同内容并发注入（单实例条带锁下最终只有一份完整数据）；模拟部分写入后重试可补齐并清理；切块参数变小后同哈希尾块被清理；同文本仅改元数据返回 `metadata_updated=true` 且不调用 embedding。
  - 编码与路由：GBK/GB18030 上传正确转码；非法字节返回 `INVALID_ENCODING`；包含空格/中文/斜杠的 `doc_id` 通过 query 参数删除成功。
  - 超时：用可控延迟的 mock provider 触发 `REQUEST_TIMEOUT` 408，随后重试成功。
- **真实 provider 测试**（手动/忽略标记）
  - 本地 fastembed：`#[ignore]`（首次需下载模型）。
  - API embedding：`KB_TEST_OPENAI_*` 环境变量 gating。
- 验收命令：`cargo fmt --check`、`cargo clippy --all-targets -- -D warnings`、`cargo test`。

---

## 13. 开发与运行

- 本地开发（Windows，无 Docker）：
  1. 下载 `qdrant-x86_64-pc-windows-msvc.zip`（v1.20.2）并解压，运行 `qdrant.exe`（gRPC 6334 / REST 6333），数据默认写入当前目录。
  2. `cp config/config.example.toml config/config.toml`，选择 embedding 配置。
  3. `cargo run -p kb-server`。
- 部署（Docker）：`docker compose up --build`（kb-server + qdrant，模型缓存与 Qdrant 数据挂载卷）。
- 前置条件：Rust stable（1.100 已验证；fastembed 实际 MSRV 1.92）、MSVC + Windows SDK（已验证本机 VS 2022 Community）、首次构建需网络下载 ONNX Runtime、首次使用本地模型需下载模型文件。
- README 必须包含：架构说明、目录结构、配置全表、API 全量示例（curl）、agent 接入示例（Python / TypeScript）、测试、扩展指南（新增解析器/新增 embedding provider）、FAQ（模型下载、HF 镜像、端口 6333/6334 区别、维度不匹配、Windows 二进制启动、Docker 部署）。

---

## 14. 已知限制与风险

| 风险/限制 | 缓解 |
|-----------|------|
| fastembed 模型别名与实际枚举不一致 | 已按 7.8.0 源码核对（见 6.2）；未知别名报错并列出支持项 |
| 首次下载本地模型慢/被墙 | 支持 `cache_dir`，README 提供 `HF_ENDPOINT` 镜像与手动放置模型说明 |
| API embedding 厂商兼容差异 | 只承诺 OpenAI 兼容协议；提供多厂商配置样例；错误信息保留上游响应片段 |
| 同维度更换 embedding 模型无法检测 | README 明确「换模型 = 重建集合」流程；payload 记录 `embedding_model` |
| 单实例写协调，多副本并发写同一文档无保证 | README 明确单写实例部署；水平扩展需外部协调，列入 roadmap |
| 请求超时取消可能留下部分新 point | 先新后清 + 每次注入对账，客户端重试即可收敛 |
| 上传字符集需显式指定，不自动探测 | 默认 UTF-8，支持 `encoding=gbk/gb18030`；README 说明 |
| 同步注入大文档耗时长 | 文档给出超时配置与批量建议；核心层预留异步 job 扩展点 |
| qdrant-client 无本地模式，测试依赖外部 Qdrant | README/设计给出 Windows 二进制与 Docker 两种启动方式；测试用 `KB_TEST_QDRANT_URL` gating |
| fastembed/ORT 构建依赖 MSVC 与网络 | 已验证本机 VS 2022 + VC Tools；README 说明构建前置与离线链接选项 |
| Qdrant 版本差异 | 固定 Qdrant 1.20.x 与客户端 1.20.0；docker-compose 固定镜像版本 |
| Windows 构建 ONNX 依赖 | 使用 fastembed 默认预编译 ONNX Runtime（静态链接）；README 提供故障排查 |

---

## 15. 留给实现者的空间

- 具体 crate 版本：关键依赖已核实（见附录 A），其余以 `cargo add` 时的最新稳定版为准，锁定 `Cargo.lock`。
- 切块细节（句子边界正则、段落合并策略）在不改变外部行为的前提下可调整。
- 日志格式、内部模块划分可微调；对外 API、配置键名、payload 字段以本文档为准。
- README 语言为中文，代码注释中文/英文均可，公开 API 文档注释用英文。

---

## 16. 验收标准

1. `cargo test` 全绿；clippy 无警告。
2. 按 README 从零启动：qdrant + 服务 + 本地模型；用 curl 完成建集合、注入、检索、删除全流程。
3. 切换为 API embedding（任意 OpenAI 兼容端点）后，同样流程可用。
4. 重启服务后数据仍在（Qdrant 持久化）。
5. 幂等：同一文档重复注入返回 `skipped=true`；修改内容后重注入，旧分块被清理。
6. README 覆盖快速开始、配置、API、扩展、排障，读者无需阅读源码即可接入。

---

## 附录 A：关键依赖版本（2026-11-25 核实）

| crate | 版本 | 说明 |
|-------|------|------|
| `qdrant-client` | 1.20.0 | 仅远程 gRPC；`Qdrant::from_url(...).build()`；`query` 替代 `search_points` |
| `fastembed` | 7.8.0 | 默认 features 含 `onnx`、`ort-download-binaries-native-tls`、`hf-hub-native-tls` |
| `ort` | 2.0.0-rc.15 | fastembed 精确锁定；构建期下载 ONNX Runtime 1.31.0 |
| `axum` | 0.8.9 | 路径参数用 `{name}`；multipart 需显式 feature；默认 body limit 2 MiB |
| `figment` | 0.10.19 | `Toml` + `Env::prefixed("KB_").split("__")`；Env 默认小写键 |
| `tokio` / `reqwest` / `serde` / `uuid` / `sha2` / `thiserror` / `tracing` / `scraper` | 最新稳定版 | 由 `cargo add` 解析并锁定 |

Qdrant 服务端：1.20.2（Windows 二进制或 Docker 镜像）。