# 可视化工作台

工作台用于浏览集合和文档、语义检索，以及查看「输入 → 解析和切块 → 嵌入 → 写入」的进度与结果。概览只读显示后台当前的嵌入模型和向量维度；文档详情保留该文档入库时使用的模型。

`kb-server` 在同一个进程中托管工作台、管理后台和 REST API。三者共用 `KnowledgeBaseService` 和写入锁。后台保存模型设置后，新提交的工作台任务和 REST 请求使用最新模型；已提交的任务继续使用原模型。

## 启动

Docker Compose 同时启动 Qdrant 与 `kb-server`：

```powershell
docker compose up -d --build
```

默认打开 `http://127.0.0.1:8090` 使用工作台，打开 `http://127.0.0.1:8080/admin/` 管理模型。升级旧 Compose 部署时，请先停掉旧的独立 `kb-dashboard` 容器，释放 8090。

Windows 本地开发可以先安装 WASM 目标与 Trunk，再运行脚本：

```powershell
rustup target add wasm32-unknown-unknown
cargo install trunk --locked
powershell -ExecutionPolicy Bypass -File .\scripts\start-dashboard.ps1
```

脚本会在缺少前端产物时构建前端，然后用同一个 `kb-server` 进程启动 API、管理端和工作台。也可以手动构建：

```powershell
Push-Location crates\kb-dashboard-ui
trunk build --release
Pop-Location
$env:DASHBOARD_PORT = "8090"
cargo run -p kb-server
```

| 变量 | 说明 |
|---|---|
| `DASHBOARD_PORT` | 非 Docker 启动时显式设置才能启用工作台；Compose 已设为 8090 |
| `DASHBOARD_HOST` | 本地监听地址，默认 `127.0.0.1` |
| `DASHBOARD_UI_DIR` | 前端产物目录，默认 `crates/kb-dashboard-ui/dist` |
| `KB_DASHBOARD_HTTP_PORT` | Compose 中发布到宿主机的端口，默认 8090 |

前端热更新时保持后台在 8090 运行，再到 `crates/kb-dashboard-ui` 执行 `trunk serve`。Trunk 在 `http://127.0.0.1:8081` 提供开发页面，同源代理 `/api` 到 8090。工作台没有登录鉴权，只用于本地或有访问控制的环境。

## 后台模型设置

配置 `KB_ADMIN__ACCESS_KEY` 后，登录 `/admin/` 并打开「设置 → 嵌入」。管理员可以选用本地模型、OpenAI 兼容 API、DashScope 原生 API 或演示 Mock，也可以测试当前模型和恢复启动配置。恢复嵌入配置不会改动重排或解析设置。供应商密钥由后台保管，不发送到工作台。

旧文档不会随模型切换自动更新。集合维度不同会报 `409 EMBEDDING_DIM_MISMATCH`；即使维度相同，不同模型的向量也不宜混用。请新建集合并重新入库，再把检索切换到新集合。

## 工作台页面

1. **概览**：查看 Qdrant 连通性、集合、点数、延迟，以及后台当前模型和维度。
2. **入库**：输入或导入 UTF-8 文本、Markdown、HTML 或 JSON；可指定文档 ID、标题、来源和标签。集合不存在时默认按当前模型维度创建。页面逐步展示切块、首条向量和写入结果。
3. **检索**：切换集合、按关键字筛查文档、运行语义检索、查看文档块和删除文档。检索展示向量分数；配置重排后也会展示重排分数。

工作台后端只在内存中保留最近 40 个入库任务，重启不保留任务进度。已写入的向量继续保存在 Qdrant。文档列表每次默认扫描 2000 个点；大型集合可能只展示一部分文档。

## 工作台 API

以下接口由 8090 工作台监听提供，响应为 JSON，错误格式为 `{"error":{"code":"...","message":"..."}}`。模型管理接口只在 8080 的 `/admin/api` 下提供，需要独立的管理密钥。

| 方法 | 路径 | 说明 |
|---|---|---|
| GET | `/api/health` | Qdrant 连接、嵌入状态、版本与运行时间 |
| GET | `/api/overview` | 集合统计及后台生效模型 |
| GET / POST | `/api/collections` | 列出集合或按当前模型维度创建集合 |
| DELETE | `/api/collections/{name}` | 删除集合 |
| GET | `/api/collections/{name}/documents?q=&limit=` | 按文档聚合列表 |
| GET / DELETE | `/api/collections/{name}/document?doc_id=` | 查看文档全部块或删除文档 |
| POST | `/api/collections/{name}/search` | 语义检索，例如 `{"query":"差旅如何报销","top_k":5}` |
| POST | `/api/pipeline/run` | 提交一次入库任务 |
| GET | `/api/pipeline/jobs/{id}` | 获取任务状态、事件和结果 |

入库示例（`POST /api/pipeline/run`）：

```json
{
  "collection": "handbook",
  "text": "# 员工手册\n\n年假需在 HR 系统提交申请。",
  "doc_id": "leave-policy",
  "format": "markdown",
  "title": "员工手册",
  "source": "handbook.md",
  "tags": ["hr"],
  "create_collection": true
}
```

收到 `202 Accepted` 和 `job_id` 后，轮询 `GET /api/pipeline/jobs/{job_id}`。完成时 `result` 包含模型、维度、块数、内容哈希、点数和预览。

以 Rust 直接接入知识库的说明见 [kb-core README](../crates/kb-core/README.md)；模型与 REST API 的完整配置见 [项目 README](../README.md)。
