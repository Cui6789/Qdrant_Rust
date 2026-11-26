# 可视化工作台（kb-dashboard）

一个 Apple 风格的 Rust 全栈工作台：Axum 后端 + Yew/WASM 前端。它直接连接 Qdrant，
复用 `kb-core` 的解析、切块、嵌入与写入逻辑，把知识库内部的每一步搬到浏览器里。
界面是单屏左右分栏：左侧切换类别并承载输入/检索，右侧展示该类别对应的数据与流程。

- **实时数据浏览器**：按 `doc_id` 聚合 Qdrant payload，展示块数、字符数、标签、模型、
  写入时间；支持关键字过滤、语义检索、查看完整块内容与删除文档。
- **运行时模型接入**：在页面中填写 Base URL、API Key、模型名，后端立即探测真实维度并
  启用；支持 OpenAI 兼容协议与 DashScope 原生协议。
- **完整入库过程**：提交文档后，解析、切块、嵌入、写入、清理逐条以真实事件回传，
  展示阶段耗时、首条向量的前 16 维、切块预览、写入前后的向量点数。
- **幂等可视化**：同一内容再次提交会命中对账流程，页面会显示“跳过嵌入与写入”，
  适合演示 `kb-core` 的幂等保证。

## 目录与架构

```
crates/
├── kb-dashboard/          # Axum 库（build_api/build_app）+ 启动器
├── kb-dashboard-types/    # 前后端共享的 serde wire types
├── kb-dashboard-ui/       # Yew/WASM 库（DashboardApp）+ 启动器
└── kb-core/               # 原有解析、切块、嵌入、Qdrant 访问层
```

- 前端通过 `ApiClient` 调用 `/api`；默认同源，`trunk serve`（8081）自动指向 8090，
  也可用 `DashboardAppProps.api_base` 指向别的后端。
- 后端直接使用 `QdrantStore`（gRPC 6334），不依赖 `kb-server` 是否运行。
- 嵌入 Key 只保存在 `kb-dashboard` 进程内存中；重启即清除，不写盘、不写日志。
- 后端保留最近 40 个入库任务；任务结束后仍可读取事件与结果。

### RAG 模块复用

直接通过 Rust 操作 RAG 数据库（建集合、入库、检索、滚动浏览、删除）的模块地图与
参考脚本见 [crates/kb-core/README.md](../crates/kb-core/README.md)。工作台本身的
启动、接口与安全说明见本文；`kb-dashboard` 也以库形式公开
`AppState::from_config`、`build_api`、`build_app`，供内部复用。

## 快速启动

前置条件：Rust stable、Qdrant、Trunk。

```powershell
# 1. 启动本地 Qdrant（下载并启动 v1.20.2；已运行则直接退出）
powershell -ExecutionPolicy Bypass -File .\scripts\start-qdrant.ps1

# 2. 一次性安装 WASM 工具链
rustup target add wasm32-unknown-unknown
cargo install trunk --locked        # 或从 GitHub Releases 下载 trunk.exe

# 3. 构建前端
cd crates\kb-dashboard-ui
trunk build --release
cd ..\..

# 4. 启动工作台
cargo run -p kb-dashboard
```

打开 <http://127.0.0.1:8090>。

也可以直接运行：

```powershell
powershell -ExecutionPolicy Bypass -File .\scripts\start-dashboard.ps1
```

该脚本会启动 Qdrant、在缺少前端产物时调用 trunk 构建，然后启动 dashboard。

### 配置项

工作台复用 `config/config.toml` 与 `KB_*` 环境变量（`AppConfig`）来读取 Qdrant 地址、
Qdrant API Key、批大小与超时。额外的环境变量：

| 变量 | 默认值 | 说明 |
|---|---|---|
| `DASHBOARD_HOST` | `127.0.0.1` | 监听地址；默认仅回环 |
| `DASHBOARD_PORT` | `8090` | 监听端口 |
| `DASHBOARD_UI_DIR` | `crates/kb-dashboard-ui/dist` | 前端产物目录 |
| `KB_CONFIG` | `config/config.toml` | 复用的应用配置文件 |
| `KB_QDRANT__URL` | `http://localhost:6334` | Qdrant **gRPC** 地址 |

命令行覆盖：`cargo run -p kb-dashboard -- --port 8091 --host 127.0.0.1 --config config/config.toml`。

### 开发模式（热更新）

```powershell
# 终端 1：后端
cargo run -p kb-dashboard

# 终端 2：前端，访问 http://127.0.0.1:8081
cd crates\kb-dashboard-ui
trunk serve
```

`ApiClient` 在 8081 端口会把 `/api` 请求指向 `http://127.0.0.1:8090/api`；后端开启了本地
CORS。生产使用请直接访问 8090，由后端托管静态文件；也可以给 `DashboardApp` 传入
`api_base` 指向自定义后端。

## 模型接入

在「模型接入」区域填写：

- **Base URL**：API 根地址（通常以 `/v1` 结尾）。
- **API Key**：可留空（例如本地 Ollama）；DashScope 等云端服务通常必填。
- **模型**：例如 `text-embedding-3-small`、`qwen3.7-text-embedding-flash`。
- **维度**：建议留空，由后端自动探测；填写时仅用于校验返回维度，不会改变模型输出。
- **接口协议**：`OpenAI 兼容 · /embeddings` 或 `DashScope 原生 · TextEmbedding`。

协议差异：

| 协议 | 实际请求 | 适用 |
|---|---|---|
| OpenAI 兼容 | `POST {base_url}/embeddings`，`Authorization: Bearer <key>` | OpenAI、通义兼容模式、SiliconFlow、Ollama、vLLM |
| DashScope 原生 | `POST {base_url}/services/embeddings/text-embedding/text-embedding`，body `{"model":...,"input":{"texts":[...]},"parameters":{"text_type":"query"或"document"}}` | 通义 MaaS 原生 TextEmbedding |

已验证的通义 MaaS 配置（Key 请自行填写，**不要提交到仓库**）：

| 入口 | Base URL | 模型 | 协议 | 维度 |
|---|---|---|---|---|
| OpenAI 兼容 | `https://maas.qianwenaiapi.com/compatible-mode/v1` | `qwen3.7-text-embedding-flash` | OpenAI 兼容 | 1024 |
| 原生 | `https://cn-beijing.maas.qianwenaiapi.com/api/v1` | `qwen3.7-text-embedding-flash` | DashScope 原生 | 1024 |

页面内置了 OpenAI、通义 MaaS（兼容）、通义原生、百炼兼容、SiliconFlow、Ollama 预设，
点击即可填入 URL、模型与协议。

### 密钥与安全

- Key 仅保存在 `kb-dashboard` 进程内存中；页面只回显末 4 位（如 `••••abcd`）。
- 重启 dashboard 后需要重新填写 Key；不会读取或写入 `config/config.toml` 中的 API Key。
- 工作台**没有登录鉴权**，且会把用户填写的 Base URL 当作外部请求目标。默认只绑定
  `127.0.0.1`；不要直接暴露到公网。若需远程访问，请放在带认证的反向代理之后。
- Qdrant 本身的 API Key 仍由 `qdrant.api_key` / `KB_QDRANT__API_KEY` 提供。

### 模型与集合维度

集合的向量维度在创建时固定。工作台在首次向不存在的集合入库时会用当前模型维度自动
创建；如果集合已存在且维度不一致，会返回 `409 EMBEDDING_DIM_MISMATCH`。更换模型时请
新建集合（例如 `docs_v2`）并重新入库。

## 页面布局

工作台是单屏左右分栏：左侧顶部是类别按钮与当前类别的输入/检索控制区，右侧是当前
类别对应的展示区。页面不再纵向堆叠多个区块，左右两栏各自滚动；窄屏下两栏改为上下排列。

1. **概览**：左侧显示连接状态、读取延迟、嵌入服务、运行时长与集合列表；右侧显示集合数、
   向量点、最大维度、当前模型与各集合柱状图，每 4 秒轮询 Qdrant。
2. **入库**：左侧是入库表单（集合、文档 ID、格式、内容、标题、来源、标签）；
   右侧是 `输入 → 解析/切块 → 向量化 → 写入` 四张阶段卡片与实时事件控制台，
   首条向量以前 16 维正负条形图展示。
   - 集合名支持直接输入；不存在时自动创建。
   - 支持粘贴文本或选择 UTF-8 文本文件（`.txt/.md/.markdown/.html/.htm/.json`）。
   - 未填文档 ID 时按 `source` 或内容哈希生成。
   - 完成后可从控制台跳到「检索」查看最新数据。
3. **检索**：左侧是集合切换、关键字过滤、语义检索输入与实时刷新开关；
   右侧显示检索结果、按文档聚合的表格以及文档块详情弹窗。
   - 默认每 3.5 秒刷新当前集合；可暂停。
   - 点击行查看全部块、标题路径、字符数、point id 与写入时间。
   - 语义检索显示余弦分数；需要先接入嵌入模型。
   - 支持删除文档（会删除该 `doc_id` 的全部块，操作不可撤销）。
   - 大集合每次最多扫描 2000 个点；界面会提示“部分扫描”。可先按关键字过滤。
4. **模型接入**：左侧是嵌入端点表单与常用服务预设；右侧显示当前配置、能力说明，
   以及「保存并测试」后的实时探针结果（真实向量、维度与延迟）。

## HTTP API

所有接口位于 `/api`，返回 JSON；错误信封为
`{"error":{"code":"...","message":"..."}}`。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | Qdrant 连接、嵌入状态、版本与运行时长 |
| GET | `/api/overview` | 集合列表、总点数、最大维度、Qdrant 延迟 |
| GET | `/api/collections` | 集合列表 |
| POST | `/api/collections` | 用当前模型维度创建集合 |
| DELETE | `/api/collections/{name}` | 删除集合 |
| GET | `/api/collections/{name}/documents?q=&limit=` | 按文档聚合的列表 |
| GET | `/api/collections/{name}/document?doc_id=` | 文档全部块 |
| DELETE | `/api/collections/{name}/document?doc_id=` | 删除文档 |
| POST | `/api/collections/{name}/search` | 语义检索 `{"query":"...","top_k":5}` |
| GET | `/api/embedding/config` | 当前嵌入状态与默认值 |
| POST | `/api/embedding/config` | 保存并探测嵌入端点 |
| DELETE | `/api/embedding/config` | 清除内存中的嵌入配置 |
| POST | `/api/embedding/mock` | 启用本地确定性演示嵌入 |
| POST | `/api/pipeline/run` | 提交一次入库任务，返回 `job_id` |
| GET | `/api/pipeline/jobs/{id}` | 任务快照（事件、结果、错误） |

### 嵌入配置请求示例

```json
{
  "base_url": "https://maas.qianwenaiapi.com/compatible-mode/v1",
  "api_key": "sk-REPLACE_ME",
  "model": "qwen3.7-text-embedding-flash",
  "dimensions": null,
  "protocol": "openai"
}
```

DashScope 原生：

```json
{
  "base_url": "https://cn-beijing.maas.qianwenaiapi.com/api/v1",
  "api_key": "sk-REPLACE_ME",
  "model": "qwen3.7-text-embedding-flash",
  "dimensions": null,
  "protocol": "dashscope"
}
```

### 入库任务请求示例

```json
{
  "collection": "qwen-docs",
  "text": "# 员工手册\n\n年假需在 HR 系统提交申请。",
  "doc_id": "handbook",
  "format": "markdown",
  "title": "员工手册",
  "source": "handbook.md",
  "tags": ["hr"],
  "create_collection": true
}
```

`POST /api/pipeline/run` 返回 `202 Accepted` 与 `{"job_id":"..."}`；随后轮询
`GET /api/pipeline/jobs/{job_id}`。任务完成后 `result` 中包含块数、内容哈希、模型、
维度、写入前后点数、切块预览与首条向量预览；失败时 `error` 给出可读原因。

## 本地演示嵌入

没有 API Key 时，可以点击「本地演示嵌入」使用 `DeterministicEmbedding`：向量由
SHA-256 派生并 L2 归一化，不访问网络。它适合演示入库流程，但语义检索没有实际意义；
正式检索请使用真实嵌入模型。

## 已知限制

- 前端构建需要 `trunk` 与 `wasm32-unknown-unknown` 目标；首次构建会编译 Yew/WebAssembly。
- 工作台是单实例、内存态：重启丢失嵌入配置与任务历史；入库逻辑本身仍由 Qdrant 持久化。
- 文档列表按 Qdrant scroll 聚合，每次最多 2000 点；超大集合的聚合是全量列表的近似。
- 集合维度切换、混合检索、rerank 等仍受 `kb-core` 现有能力限制（见根 README）。
