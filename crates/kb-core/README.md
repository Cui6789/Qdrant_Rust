# kb-core：RAG 数据库模块复用指南

`kb-core` 是 Qdrant_Rust 的 RAG 数据层：负责文档解析、标题感知切块、Embedding、
确定性 point ID、幂等对账、写入 Qdrant、相似度检索和按文档删除。它不依赖
`kb-server` 或可视化工作台，可以直接嵌入任意 Rust 异步服务。

这份 README 面向“通过代码直接操作 RAG 数据库”的场景。完整的可运行脚本见
[`examples/rag_operations.rs`](examples/rag_operations.rs)：

```powershell
# 先启动 Qdrant（gRPC 6334），然后：
cargo run -p kb-core --example rag_operations
```

示例使用离线确定性嵌入，会创建 `rag-example-<pid>` 集合、入库、检索、滚动浏览，
最后删除文档和集合。

## 1. 两层 API

| 层 | 入口 | 适合 |
|---|---|---|
| 高层 RAG | `KnowledgeBaseService` | 入库、更新、检索、删除文档；自动处理解析、切块、Embedding、对账与清理 |
| 低层存储 | `QdrantStore` | 直接 count/scroll/filter/upsert/delete 原生 Qdrant point |
| Embedding | `EmbeddingProvider` | 替换为 API、本地 ONNX 或测试用嵌入实现 |

一般优先使用 `KnowledgeBaseService`；只有在需要自定义 payload、分页扫描或运维操作
时才直接使用 `QdrantStore`。

## 2. 模块地图

| 模块 | 内容 |
|---|---|
| `config` | `AppConfig`、`QdrantConfig`、`EmbeddingConfig`、`LocalEmbeddingConfig`、`ApiEmbeddingConfig`、`IngestConfig` |
| `models` | `DocumentFormat`、`DocumentMetadata`、`SearchRequest`、`SearchFilter`、`SearchHit`、`IngestResult`、`CollectionInfo` |
| `ingest::parser` | `parse_text`：文本/HTML/JSON 归一化 |
| `ingest::chunker` | `chunk_document`：标题感知切块 |
| `embedding` | `EmbeddingProvider` trait；`api::ApiEmbedding`、`local::LocalEmbedding`、`mock::DeterministicEmbedding` |
| `store` | `QdrantStore`、`StoredPoint`；并重导出 `Filter`、`Condition`、`MatchValue`、`PointStruct`、`ScoredPoint`、`Payload` |
| `service` | `KnowledgeBaseService`、`IngestInput`、`IngestEvent`、`IngestResult`；确定性 `point_id` 与 `resolve_doc_id` |
| `error` | `KbError` 与 `Result<T>` |

## 3. 依赖与 feature

```toml
[dependencies]
kb-core = { path = "../kb-core" }
tokio = { version = "1", features = ["full"] }
serde_json = "1"
```

`kb-core` 默认启用 `local-embedding`（fastembed / ONNX）。如果只需要 API 嵌入，
可以关闭默认 feature，避免编译本地模型：

```toml
kb-core = { path = "../kb-core", default-features = false }
```

直接使用 `Filter` / `Condition` / `PointStruct` 等 Qdrant 类型时，不需要额外添加
`qdrant-client` 依赖，从 `kb_core::store` 导入即可。

## 4. 初始化

```rust
use std::sync::Arc;
use std::time::Duration;

use kb_core::config::{IngestConfig, QdrantConfig};
use kb_core::embedding::mock::DeterministicEmbedding;
use kb_core::service::KnowledgeBaseService;
use kb_core::store::QdrantStore;

let store = Arc::new(QdrantStore::new(&QdrantConfig::default())?);
store.wait_ready(Duration::from_secs(10)).await?;

let embedder = Arc::new(DeterministicEmbedding::new(64));
let service = KnowledgeBaseService::new(store.clone(), embedder, IngestConfig::default());
```

- `QdrantConfig::default()` 连接 `http://localhost:6334`（Qdrant gRPC，不是 REST 6333）。
- `IngestConfig::default()`：`chunk_size=800`、`chunk_overlap=120`、`min_chunk_size=30`、
  `upsert_batch_size=64`。
- `store` 与 `service` 都持有 `Arc<QdrantStore>`，可以按需克隆。

### 真实 Embedding

OpenAI 兼容端点：

```rust
use std::sync::Arc;
use kb_core::config::ApiEmbeddingConfig;
use kb_core::embedding::api::ApiEmbedding;
use kb_core::embedding::EmbeddingProvider;

let embedder: Arc<dyn EmbeddingProvider> = Arc::new(
    ApiEmbedding::new(
        &ApiEmbeddingConfig {
            base_url: "https://maas.qianwenaiapi.com/compatible-mode/v1".to_string(),
            api_key: std::env::var("EMBEDDING_API_KEY")?,
            model: "qwen3.7-text-embedding-flash".to_string(),
            // dimensions = 0 时，构造阶段会真实请求一次以探测维度
            ..Default::default()
        },
        32, // batch_size
        4,  // max_concurrent_batches
    )
    .await?,
);
```

本地 fastembed 模型：

```rust
#[cfg(feature = "local-embedding")]
let embedder: Arc<dyn EmbeddingProvider> = {
    use kb_core::embedding::local::LocalEmbedding;
    let local_config = kb_core::config::LocalEmbeddingConfig::default();
    let batch_size = 32;
    // LocalEmbedding::new 可能下载/加载 ONNX 模型，异步服务中建议 spawn_blocking。
    Arc::new(tokio::task::spawn_blocking(move || {
        LocalEmbedding::new(&local_config, batch_size)
    }).await??)
};
```

也可以使用 `AppConfig::load(None)?` 从 `config/config.toml` 和 `KB_*` 环境变量读取
`qdrant`、`embedding`、`ingest` 配置，再按 `ProviderKind` 构造 provider。
`kb-server` 的 `build_embedder` 就是这种选择逻辑的参考实现。

## 5. 集合操作

```rust
let collections = service.list_collections().await?;
for collection in &collections {
    println!("{}: {} points, {} dims", collection.name, collection.points_count, collection.vector_size);
}

let name = "docs";
if !store.collection_exists(name).await? {
    // 使用当前 embedder 的维度创建集合
    let info = service.create_collection(name).await?;
    println!("created {} at {} dims", info.name, info.vector_size);
}

let info = store.collection_info(name).await?;
println!("{} points", info.points_count);
```

- 集合名规则：`^[a-zA-Z0-9][a-zA-Z0-9_-]{0,63}$`。
- 集合维度在创建时固定；更换 embedding 模型必须新建集合。
- `service.create_collection` 在集合已存在时返回 `KbError::CollectionExists`，所以先
  用 `store.collection_exists` 判断。
- 删除集合：`service.delete_collection("docs").await?;`（不可撤销）。

## 6. 写入、更新与幂等

```rust
use kb_core::models::{DocumentFormat, DocumentMetadata};
use kb_core::service::IngestInput;

let input = IngestInput {
    doc_id: Some("handbook".to_string()), // 建议显式指定，方便后续更新/删除
    text: "# 员工手册\n\n年假需要在 HR 系统提交申请。".to_string(),
    format: DocumentFormat::Markdown,
    metadata: DocumentMetadata {
        source: Some("handbook.md".to_string()),
        title: Some("员工手册".to_string()),
        tags: vec!["hr".to_string(), "policy".to_string()],
        extra: serde_json::Map::new(),
    },
};

let result = service.ingest_document("docs", input.clone()).await?;
println!(
    "doc_id={} chunks={} skipped={} metadata_updated={} model={}",
    result.doc_id,
    result.chunks,
    result.skipped,
    result.metadata_updated,
    result.embedding_model
);

// 文本不变、只改 metadata：payload 原地更新，不会重新 embedding。
let mut metadata_update = input.clone();
metadata_update.metadata.tags.push("updated".to_string());
let updated = service.ingest_document("docs", metadata_update).await?;
println!("metadata_updated={}", updated.metadata_updated);
```

- `doc_id` 可省略：服务会按 `metadata.source` 或内容哈希生成 `doc-<hash16>`。
- `DocumentFormat`：`Text`、`Markdown`、`Html`、`Json`；也可用
  `DocumentFormat::infer_from_filename("handbook.md")`。
- 从文件入库时：

```rust
let path = std::path::Path::new("handbook.md");
let text = std::fs::read_to_string(path)?; // 非 UTF-8 文件请先解码
let format = DocumentFormat::infer_from_filename(path.to_string_lossy().as_ref());
```

- Markdown 标题会进入 `heading_path` 并参与切块；HTML/JSON 会先归一化为纯文本。
- `metadata.extra` 是任意 `serde_json::Map`，会原样写入 payload。

重复入库的对账行为：

| 情况 | 行为 | `IngestResult` |
|---|---|---|
| 文本与 metadata 都相同 | 跳过 embedding 与写入 | `skipped=true` |
| 文本相同、metadata 改变 | 原地更新 payload | `metadata_updated=true` |
| 文本改变 | 计算新 hash、写入新版本并清理旧块 | `skipped=false` |
| 上次写入不完整 / 有多余块 | 重新写入并清理到当前版本 | `skipped=false` |

需要观察阶段进度时使用：

```rust
service
    .ingest_document_observed("docs", input, &|event| {
        println!("{event:?}");
    })
    .await?;
```

批量入库只需循环调用；同一个 `collection + doc_id` 的写入由进程内 striped lock
串行化，避免并发入库互相覆盖。该锁是单实例保证，多实例部署需要外部协调。

## 7. 相似度检索

```rust
use kb_core::models::{SearchFilter, SearchRequest};

let hits = service
    .search(
        "docs",
        SearchRequest {
            query: "年假怎么申请".to_string(),
            top_k: 5,
            score_threshold: None,
            filter: Some(SearchFilter {
                doc_ids: Some(vec!["handbook".to_string()]),
                tags_any: Some(vec!["hr".to_string()]),
                source: None,
            }),
        },
    )
    .await?;

for hit in hits {
    println!(
        "score={:.4} doc={} chunk={} heading={} text={}",
        hit.score,
        hit.doc_id,
        hit.chunk_index,
        hit.metadata.heading_path,
        hit.text
    );
}
```

- `top_k` 必须在 `1..=50`；`query` 不能为空。
- `SearchFilter` 的不同字段是 AND 关系；`tags_any` 在标签数组中匹配任一标签。
- 检索会用当前 provider 的 `embed_query` 编码问题；集合必须使用同一模型/维度。
- `SearchHit` 包含 `score`、`doc_id`、`chunk_index`、`text` 和 `metadata`
  （`source`、`title`、`tags`、`heading_path`、`extra`）。
- `DeterministicEmbedding` 只适合离线演示/测试，不具备真实语义。

## 8. 直接浏览、统计与筛选

```rust
use std::collections::BTreeMap;
use kb_core::store::{Condition, Filter, MatchValue};

// 扫描一页 point（payload 中包含 doc_id、chunk_index、chunk_count、text 等）
let points = store.scroll_points("docs", None, 2_000).await?;

let mut documents: BTreeMap<String, usize> = BTreeMap::new();
for point in &points {
    if let Some(doc_id) = point.payload.get("doc_id").and_then(|value| value.as_str()) {
        *documents.entry(doc_id.to_string()).or_default() += 1;
    }
}
println!("documents: {documents:?}");

// 精确统计
let total = store.count("docs", None).await?;

// 按 doc_id 过滤
let filter = Filter::must([Condition::matches(
    "doc_id",
    MatchValue::Keyword("handbook".to_string()),
)]);
let handbook_points = store.count("docs", Some(filter.clone())).await?;
println!("total={total} handbook={handbook_points}");
```

常用 payload 字段：

| 字段 | 说明 |
|---|---|
| `doc_id` | 文档标识，同一文档的所有块共享 |
| `chunk_index` / `chunk_count` | 块序号与总数 |
| `text` | 当前块文本 |
| `content_hash` | 归一化文本的 SHA-256 |
| `source` / `title` / `tags` | 调用方提供的 metadata |
| `heading_path` | Markdown 标题路径（可能为空） |
| `extra` | 调用方任意 JSON 对象 |
| `embedding_model` | 生成向量的模型标识 |
| `created_at` | 写入时间（RFC 3339） |

`scroll_points(collection, filter, limit)` 当前只返回**一页**结果；更大的集合请按
业务字段缩小过滤范围，或直接使用 `qdrant-client` 的 `next_page_offset` 分页 API。
`scroll_payloads` 只返回 payload，`scroll_ids` 只返回 UUID。

## 9. 删除

```rust
// 删除一个文档的全部块，返回删除的 point 数
let deleted = service.delete_document("docs", "handbook").await?;
println!("deleted {deleted} points");

// 按自定义 Qdrant filter 删除
store
    .delete_by_filter(
        "docs",
        Filter::must([Condition::matches(
            "source",
            MatchValue::Keyword("handbook.md".to_string()),
        )]),
    )
    .await?;

// 删除整个集合
service.delete_collection("docs").await?;
```

删除不可撤销；`delete_document` 会删除该 `doc_id` 的所有块。

## 10. 低层 `QdrantStore` 方法

| 方法 | 说明 |
|---|---|
| `QdrantStore::new(&QdrantConfig)` | 创建 gRPC 客户端 |
| `wait_ready(Duration)` | 轮询直到 Qdrant 可用 |
| `list_collections()` / `collection_info(name)` / `collection_exists(name)` | 集合元数据 |
| `create_collection(name, dim)` / `delete_collection(name)` | 原生集合操作 |
| `count(collection, Option<Filter>)` | 精确计数 |
| `scroll_points(collection, Option<Filter>, limit)` | point id + payload |
| `scroll_payloads(collection, Filter, limit)` | 仅 payload |
| `scroll_ids(collection, Filter, limit)` | 仅 UUID point id |
| `search(collection, vector, limit, threshold, filter)` | 原生向量查询，返回 `ScoredPoint` |
| `upsert(collection, Vec<PointStruct>)` | 原生写入，`wait(true)` |
| `delete_by_filter(collection, Filter)` | 按 filter 删除 |
| `set_metadata_payload(collection, Filter, Map)` | 原地更新 metadata |

`kb_core::store` 重导出了 `Filter`、`Condition`、`MatchValue`、`PointStruct`、
`ScoredPoint`、`Payload`，一般无需直接依赖 `qdrant-client`。如果要做自定义
`PointStruct` 写入，payload 应至少包含 `doc_id`、`chunk_index`、`content_hash`、
`text` 等字段，否则后续 `KnowledgeBaseService` 的对账/检索无法正确识别。

## 11. 错误与常见约束

```rust
use kb_core::error::KbError;

match service.search("docs", request).await {
    Ok(hits) => { /* ... */ }
    Err(KbError::CollectionNotFound(name)) => eprintln!("集合不存在: {name}"),
    Err(KbError::Validation(message)) => eprintln!("参数错误: {message}"),
    Err(KbError::DimensionMismatch(message)) => eprintln!("维度不匹配: {message}"),
    Err(other) => eprintln!("其他错误: {other}"),
}
```

- Qdrant 使用 **gRPC 端口 6334**；REST 6333 只用于 Web Dashboard/管理接口。
- 集合维度固定；换模型请新建集合并重新入库。
- `kb-core` 的写入并发保证是**单进程**的；多实例写入需要外部锁/单写路由。
- `local-embedding` 首次运行可能需要下载 ONNX 模型；可用 `HF_ENDPOINT`/`HF_HOME`
  或 `LocalEmbeddingConfig.cache_dir` 控制缓存。
- API embedding 的 `dimensions=0` 会在构造时探测真实维度；显式设置时只用于校验/请求。
- 任何写操作返回成功都代表 Qdrant 已 `wait(true)` 应用，不需要额外轮询。

## 12. 完整示例

[`examples/rag_operations.rs`](examples/rag_operations.rs) 把上面的初始化、建集合、
入库、幂等重入、检索、scroll 聚合、filter 计数、删除文档和删除集合串成一条可直接
运行的脚本：

```powershell
cargo run -p kb-core --example rag_operations
```

它使用 `DeterministicEmbedding::new(64)`，不访问网络；真实项目替换为
`ApiEmbedding` 或 `LocalEmbedding` 即可。
