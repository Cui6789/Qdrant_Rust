use gloo_timers::future::TimeoutFuture;
use kb_dashboard_types::{
    CollectionSummary, JobStatus, PipelineChunkPreview, PipelineEvent, PipelineJob, PipelineRequest,
};
use serde_json::Value;
use wasm_bindgen_futures::{spawn_local, JsFuture};
use web_sys::{HtmlInputElement, HtmlSelectElement, HtmlTextAreaElement};
use yew::prelude::*;

use crate::api;
use crate::components::ui::{SectionHeading, Spinner, VectorBars};
use crate::icons;
use crate::util::{
    format_clock, format_duration, format_int, infer_format, scroll_to, stem, truncate,
};

const SAMPLE_DOCUMENT: &str = r#"# 向量工作台

Qdrant_Rust 把「文档解析 → 切块 → 向量化 → 写入 Qdrant」封装成一条稳定的 Rust 管道。这个页面把管道内部的每一步展开，让你在浏览器里观察文本如何变成向量，再变成一条可以被检索的 point。

## 为什么需要可视化

向量数据库的核心对象不可见：你不能像翻文件夹一样直接看到 512 个浮点数。可视化把不可见的状态拆成几个可以理解的阶段——原始文本、切块结果、嵌入输出与写入确认。每个阶段都有真实的耗时和数值。

## 切块与幂等

Markdown 按标题切块，并保留 heading_path。相同内容使用确定性 point id，重复提交会走对账流程：如果文本与元数据都未变化，服务会跳过 embedding，避免浪费 API 调用。

## 一次完整的旅程

输入一段文字，选择集合，点击开始。你会看到解析字符数、切块数量、首块向量与写入批次数依次出现。完成后，右侧数据浏览器会立刻展示这篇文档的真实块内容。

把这段示例重新提交一次，就能看到幂等对账如何跳过嵌入与写入。"#;

#[derive(Properties, PartialEq)]
pub struct PipelineProps {
    pub collections: Vec<CollectionSummary>,
    pub active_collection: Option<String>,
    pub embedding_configured: bool,
    pub on_collection: Callback<String>,
    pub on_done: Callback<()>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum StageState {
    Pending,
    Running,
    Done,
    Skipped,
    Error,
}

#[derive(Debug, Clone, PartialEq)]
struct StageView {
    state: StageState,
    detail: String,
    duration_ms: Option<u64>,
}

impl StageState {
    fn class(self) -> &'static str {
        match self {
            Self::Pending => "is-pending",
            Self::Running => "is-running",
            Self::Done => "is-done",
            Self::Skipped => "is-skipped",
            Self::Error => "is-error",
        }
    }

    fn label(self) -> &'static str {
        match self {
            Self::Pending => "等待",
            Self::Running => "进行中",
            Self::Done => "完成",
            Self::Skipped => "已跳过",
            Self::Error => "失败",
        }
    }
}

#[function_component(PipelineSection)]
pub fn pipeline_section(props: &PipelineProps) -> Html {
    let initial_collection = props
        .active_collection
        .clone()
        .or_else(|| {
            props
                .collections
                .first()
                .map(|collection| collection.name.clone())
        })
        .unwrap_or_default();
    let collection = use_state(move || initial_collection);
    let text = use_state(String::new);
    let doc_id = use_state(String::new);
    let format = use_state(|| "markdown".to_string());
    let title = use_state(String::new);
    let source = use_state(String::new);
    let tags = use_state(String::new);
    let submitting = use_state(|| false);
    let job = use_state(|| None::<PipelineJob>);
    let error = use_state(|| None::<String>);
    let log_ref = use_node_ref();

    {
        let log_ref = log_ref.clone();
        let len = job.as_ref().map(|job| job.events.len()).unwrap_or(0);
        use_effect_with(len, move |_| {
            if let Some(element) = log_ref.cast::<web_sys::Element>() {
                element.set_scroll_top(element.scroll_height());
            }
            || ()
        });
    }

    {
        let collection = collection.clone();
        let active = props.active_collection.clone();
        use_effect_with(active, move |active| {
            if let Some(name) = active {
                if !name.is_empty() {
                    collection.set(name.clone());
                }
            }
            || ()
        });
    }
    {
        let collection = collection.clone();
        let names: Vec<String> = props
            .collections
            .iter()
            .map(|entry| entry.name.clone())
            .collect();
        use_effect_with(names, move |names| {
            if collection.trim().is_empty() {
                if let Some(first) = names.first() {
                    collection.set(first.clone());
                }
            }
            || ()
        });
    }

    let on_submit = {
        let collection = collection.clone();
        let text = text.clone();
        let doc_id = doc_id.clone();
        let format = format.clone();
        let title = title.clone();
        let source = source.clone();
        let tags = tags.clone();
        let submitting = submitting.clone();
        let job = job.clone();
        let error = error.clone();
        let on_collection = props.on_collection.clone();
        let on_done = props.on_done.clone();
        let embedding_configured = props.embedding_configured;
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            if *submitting {
                return;
            }
            let collection_name = collection.trim().to_string();
            let content = text.trim().to_string();
            if collection_name.is_empty() {
                error.set(Some("请填写集合名称".to_string()));
                return;
            }
            if content.is_empty() {
                error.set(Some("请粘贴文档内容或选择一个文本文件".to_string()));
                return;
            }
            if !embedding_configured {
                error.set(Some("请先在「模型接入」中配置嵌入服务".to_string()));
                return;
            }
            let tag_list = tags
                .split(',')
                .map(str::trim)
                .filter(|tag| !tag.is_empty())
                .map(str::to_string)
                .collect::<Vec<_>>();
            let request = PipelineRequest {
                collection: collection_name.clone(),
                text: content,
                doc_id: optional(&doc_id),
                format: Some((*format).clone()),
                title: optional(&title),
                source: optional(&source),
                tags: tag_list,
                extra: Value::Null,
                create_collection: Some(true),
            };
            submitting.set(true);
            error.set(None);
            job.set(None);
            let submitting = submitting.clone();
            let job = job.clone();
            let error = error.clone();
            let on_collection = on_collection.clone();
            let on_done = on_done.clone();
            spawn_local(async move {
                match api::start_pipeline(&request).await {
                    Ok(started) => {
                        let id = started.job_id;
                        let mut saw_finish = false;
                        for _ in 0..2400 {
                            match api::pipeline_job(&id).await {
                                Ok(snapshot) => {
                                    let finished = snapshot.status.is_finished();
                                    let failed = snapshot.status == JobStatus::Failed;
                                    job.set(Some(snapshot));
                                    if finished {
                                        saw_finish = true;
                                        if !failed {
                                            on_collection.emit(collection_name.clone());
                                            on_done.emit(());
                                        }
                                        break;
                                    }
                                }
                                Err(failure) => {
                                    error.set(Some(failure.message));
                                    break;
                                }
                            }
                            TimeoutFuture::new(300).await;
                        }
                        if !saw_finish && error.is_none() {
                            error.set(Some(
                                "任务轮询超时；可稍后在数据浏览器中确认写入结果".to_string(),
                            ));
                        }
                    }
                    Err(failure) => error.set(Some(failure.message)),
                }
                submitting.set(false);
            });
        })
    };

    let on_file = {
        let text = text.clone();
        let title = title.clone();
        let source = source.clone();
        let format = format.clone();
        let error = error.clone();
        Callback::from(move |event: web_sys::Event| {
            let input: HtmlInputElement = event.target_unchecked_into();
            let Some(file) = input.files().and_then(|files| files.get(0)) else {
                return;
            };
            if file.size() > 4.0 * 1024.0 * 1024.0 {
                error.set(Some(
                    "为了在浏览器中顺畅预览，请选择小于 4 MB 的文本文件".to_string(),
                ));
                return;
            }
            let name = file.name();
            let detected = infer_format(&name).to_string();
            let inferred_title = stem(&name);
            let text = text.clone();
            let title = title.clone();
            let source = source.clone();
            let format = format.clone();
            let error = error.clone();
            spawn_local(async move {
                match JsFuture::from(file.text()).await {
                    Ok(value) => {
                        if let Some(content) = value.as_string() {
                            text.set(content);
                            source.set(name);
                            if title.trim().is_empty() {
                                title.set(inferred_title);
                            }
                            format.set(detected);
                            error.set(None);
                        } else {
                            error.set(Some("无法读取该文件：不是有效的 UTF-8 文本".to_string()));
                        }
                    }
                    Err(_) => {
                        error.set(Some("读取文件失败，请改用粘贴文本".to_string()));
                    }
                }
            });
            input.set_value("");
        })
    };

    let on_sample = {
        let text = text.clone();
        let title = title.clone();
        let source = source.clone();
        let format = format.clone();
        let doc_id = doc_id.clone();
        Callback::from(move |_| {
            text.set(SAMPLE_DOCUMENT.to_string());
            title.set("向量工作台 · 示例文档".to_string());
            source.set("studio-demo.md".to_string());
            format.set("markdown".to_string());
            doc_id.set("studio-demo".to_string());
        })
    };

    let on_collection_input = {
        let collection = collection.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            collection.set(input.value());
        })
    };
    let on_text_input = {
        let text = text.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlTextAreaElement = event.target_unchecked_into();
            text.set(input.value());
        })
    };
    let on_doc_id_input = {
        let doc_id = doc_id.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            doc_id.set(input.value());
        })
    };
    let on_format_change = {
        let format = format.clone();
        Callback::from(move |event: web_sys::Event| {
            let select: HtmlSelectElement = event.target_unchecked_into();
            format.set(select.value());
        })
    };
    let on_title_input = {
        let title = title.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            title.set(input.value());
        })
    };
    let on_source_input = {
        let source = source.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            source.set(input.value());
        })
    };
    let on_tags_input = {
        let tags = tags.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            tags.set(input.value());
        })
    };

    let views = stage_views(job.as_ref());
    let any_running = views.iter().any(|view| view.state == StageState::Running);
    let stage_meta = [
        ("01", "输入", "文本 / Markdown / HTML / JSON"),
        ("02", "解析与切块", "标题感知 · 中文友好"),
        ("03", "向量化", "OpenAI 兼容 /embeddings"),
        ("04", "写入 Qdrant", "确定性 ID · 幂等对账"),
    ];
    let stage_icons = [
        icons::file(),
        icons::layers(),
        icons::spark(),
        icons::database(),
    ];
    let char_count = text.chars().count();
    let can_submit = props.embedding_configured
        && !*submitting
        && !collection.trim().is_empty()
        && !text.trim().is_empty();

    let on_view_browser = { Callback::from(move |_| scroll_to("browser")) };
    let on_reset = {
        let job = job.clone();
        let error = error.clone();
        Callback::from(move |_| {
            job.set(None);
            error.set(None);
        })
    };

    html! {
        <section id="pipeline" class="section pipeline-section">
            <div class="container">
                <SectionHeading
                    eyebrow={"Ingest Pipeline".to_string()}
                    title={html! { <>{"从一段文字，到一条"}<span class="serif-accent">{"向量"}</span>{"。"}</> }}
                    subtitle={html! { "每一次解析、切块、嵌入与写入都会实时回传。看到的不是示意动画，而是真实后端进度。" }}
                    centered={true}
                />

                <div class="stage-track reveal">
                    <div class="stage-track-line" aria-hidden="true">
                        <span class={classes!("track-pulse", any_running.then_some("is-active"))}></span>
                    </div>
                    { for stage_meta.iter().enumerate().map(|(index, (number, title, subtitle))| {
                        let view = views.get(index).cloned().unwrap_or(StageView {
                            state: StageState::Pending,
                            detail: "等待提交".to_string(),
                            duration_ms: None,
                        });
                        render_stage_card(
                            number,
                            title,
                            subtitle,
                            stage_icons[index].clone(),
                            &view,
                        )
                    }) }
                </div>

                <div class="pipeline-grid">
                    <form class="ingest-card reveal" onsubmit={on_submit}>
                        <div class="ingest-head">
                            <div>
                                <p class="card-kicker">{ "Ingest" }</p>
                                <h3>{ "写入一篇文档" }</h3>
                            </div>
                            <span class="char-count">{ format!("{} 字符", format_int(char_count as u64)) }</span>
                        </div>

                        { if !props.embedding_configured {
                            html! {
                                <div class="notice is-warning compact">
                                    <span class="notice-icon">{ icons::key() }</span>
                                    <div>
                                        <strong>{ "先接入嵌入模型" }</strong>
                                        <p>{ "填写 API URL 与 Key，或启用本地演示嵌入。" }</p>
                                    </div>
                                    <a class="button button-small" href="#config">{ "去接入" }</a>
                                </div>
                            }
                        } else { Html::default() } }

                        <label class="field">
                            <span class="field-label">{ "集合" }</span>
                            <span class="field-hint">{ "不存在时会自动创建，维度取自当前嵌入模型" }</span>
                            <input
                                class="input"
                                type="text"
                                list="pipeline-collections"
                                placeholder="docs"
                                value={(*collection).clone()}
                                oninput={on_collection_input}
                                spellcheck="false"
                                autocomplete="off"
                            />
                            <datalist id="pipeline-collections">
                                { for props.collections.iter().map(|entry| html! {
                                    <option value={entry.name.clone()}></option>
                                }) }
                            </datalist>
                        </label>

                        { if !props.collections.is_empty() {
                            html! {
                                <div class="quick-collections">
                                    <span class="quick-label">{ "已有集合" }</span>
                                    { for props.collections.iter().take(6).map(|entry| {
                                        let name = entry.name.clone();
                                        let target = name.clone();
                                        let collection = collection.clone();
                                        let on_collection = props.on_collection.clone();
                                        html! {
                                            <button type="button"
                                                class={classes!("chip", "chip-button", (*collection == name).then_some("is-active"))}
                                                onclick={Callback::from(move |_| {
                                                    collection.set(target.clone());
                                                    on_collection.emit(target.clone());
                                                })}>
                                                { entry.name.clone() }
                                                <span class="chip-count">{ format_int(entry.points_count) }</span>
                                            </button>
                                        }
                                    }) }
                                </div>
                            }
                        } else { Html::default() } }

                        <div class="field-row">
                            <label class="field">
                                <span class="field-label">{ "文档 ID（可选）" }</span>
                                <input
                                    class="input"
                                    type="text"
                                    placeholder="留空则按内容哈希生成"
                                    value={(*doc_id).clone()}
                                    oninput={on_doc_id_input}
                                    spellcheck="false"
                                />
                            </label>
                            <label class="field field-small">
                                <span class="field-label">{ "格式" }</span>
                                <select class="input select" onchange={on_format_change}>
                                    <option value="markdown" selected={*format == "markdown"}>{ "Markdown" }</option>
                                    <option value="text" selected={*format == "text"}>{ "纯文本" }</option>
                                    <option value="html" selected={*format == "html"}>{ "HTML" }</option>
                                    <option value="json" selected={*format == "json"}>{ "JSON" }</option>
                                </select>
                            </label>
                        </div>

                        <label class="field">
                            <span class="field-label">{ "文档内容" }</span>
                            <textarea
                                class="input textarea"
                                rows="12"
                                placeholder="粘贴一段文本，或从右侧载入示例…"
                                value={(*text).clone()}
                                oninput={on_text_input}
                                spellcheck="false"
                            ></textarea>
                        </label>

                        <div class="textarea-tools">
                            <button type="button" class="button button-ghost button-small" onclick={on_sample}>
                                { icons::spark() }{ "载入示例" }
                            </button>
                            <label class="button button-ghost button-small file-button">
                                { icons::upload() }{ "选择文件" }
                                <input
                                    type="file"
                                    class="file-input"
                                    accept=".txt,.md,.markdown,.html,.htm,.json,text/plain,text/markdown,application/json"
                                    onchange={on_file}
                                />
                            </label>
                            <span class="tool-hint">{ "浏览器内读取 UTF-8 文本，不会上传原始文件" }</span>
                        </div>

                        <div class="field-row">
                            <label class="field">
                                <span class="field-label">{ "标题（可选）" }</span>
                                <input
                                    class="input"
                                    type="text"
                                    placeholder="员工手册"
                                    value={(*title).clone()}
                                    oninput={on_title_input}
                                />
                            </label>
                            <label class="field">
                                <span class="field-label">{ "来源（可选）" }</span>
                                <input
                                    class="input"
                                    type="text"
                                    placeholder="handbook.md"
                                    value={(*source).clone()}
                                    oninput={on_source_input}
                                    spellcheck="false"
                                />
                            </label>
                        </div>

                        <label class="field">
                            <span class="field-label">{ "标签（可选，逗号分隔）" }</span>
                            <input
                                class="input"
                                type="text"
                                placeholder="hr, policy"
                                value={(*tags).clone()}
                                oninput={on_tags_input}
                            />
                        </label>

                        { if let Some(message) = &*error {
                            html! { <p class="form-message is-error" role="alert">{ message.clone() }</p> }
                        } else { Html::default() } }

                        <div class="form-actions ingest-actions">
                            <button class="button button-primary button-wide" type="submit" disabled={!can_submit}>
                                { if *submitting { html! { <Spinner /> } } else { icons::play() } }
                                { if *submitting { "正在入库…" } else { "开始入库" } }
                            </button>
                            <p class="ingest-hint">
                                { "相同内容再次提交会走幂等对账，不会重复消耗嵌入调用。" }
                            </p>
                        </div>
                    </form>

                    <div class="console-card reveal">
                        { render_console(
                            &job,
                            &error,
                            props.embedding_configured,
                            on_view_browser,
                            on_reset,
                            log_ref,
                            *submitting,
                        ) }
                    </div>
                </div>
            </div>
        </section>
    }
}

fn optional(handle: &UseStateHandle<String>) -> Option<String> {
    let value = handle.trim().to_string();
    (!value.is_empty()).then_some(value)
}

fn latest_event<'a>(job: &'a PipelineJob, stages: &[&str]) -> Option<&'a PipelineEvent> {
    job.events
        .iter()
        .rev()
        .find(|event| stages.contains(&event.stage.as_str()))
}

fn stage_views(job: Option<&PipelineJob>) -> Vec<StageView> {
    let Some(job) = job else {
        return vec![
            StageView {
                state: StageState::Pending,
                detail: "等待提交".to_string(),
                duration_ms: None,
            },
            StageView {
                state: StageState::Pending,
                detail: "等待解析".to_string(),
                duration_ms: None,
            },
            StageView {
                state: StageState::Pending,
                detail: "等待向量化".to_string(),
                duration_ms: None,
            },
            StageView {
                state: StageState::Pending,
                detail: "等待写入".to_string(),
                duration_ms: None,
            },
        ];
    };
    let status = job.status;
    let running = matches!(status, JobStatus::Queued | JobStatus::Running);
    let failed_stage = if status == JobStatus::Failed {
        job.events
            .iter()
            .rev()
            .find(|event| event.stage != "error")
            .map(|event| event.stage.clone())
            .unwrap_or_default()
    } else {
        String::new()
    };
    let input = latest_event(job, &["input"]);
    let chunk = latest_event(job, &["chunk"]);
    let parse = latest_event(job, &["parse"]);
    let reconcile = latest_event(job, &["reconcile"]);
    let embed_done = job
        .events
        .iter()
        .any(|event| event.stage == "embed" && event.data.get("preview").is_some());
    let embed_started = job.events.iter().any(|event| event.stage == "embed");
    let write_events: Vec<&PipelineEvent> = job
        .events
        .iter()
        .filter(|event| event.stage == "write")
        .collect();
    let cleanup = latest_event(job, &["cleanup"]);
    let skipped = job
        .result
        .as_ref()
        .map(|result| result.skipped)
        .unwrap_or(false)
        || reconcile.is_some();
    let failed = |stages: &[&str]| stages.contains(&failed_stage.as_str());
    let input_ms = input.map(|event| event.elapsed_ms).unwrap_or(0);
    let chunk_duration = chunk
        .or(parse)
        .map(|event| event.elapsed_ms.saturating_sub(input_ms));
    let embed_start_event = job
        .events
        .iter()
        .find(|event| event.stage == "embed" && event.data.get("preview").is_none());
    let embed_end_event = job
        .events
        .iter()
        .rev()
        .find(|event| event.stage == "embed" && event.data.get("preview").is_some());
    let embed_duration = match (embed_start_event, embed_end_event) {
        (Some(start), Some(end)) => Some(end.elapsed_ms.saturating_sub(start.elapsed_ms)),
        _ => None,
    };
    let write_start_event = write_events.first().copied();
    let write_end_event = cleanup.or_else(|| write_events.last().copied());
    let write_duration = match (write_start_event, write_end_event) {
        (Some(start), Some(end)) if end.seq >= start.seq => {
            Some(end.elapsed_ms.saturating_sub(start.elapsed_ms))
        }
        _ => None,
    };

    let input_state = if failed(&["input"]) {
        StageState::Error
    } else if input.is_some() {
        StageState::Done
    } else if running {
        StageState::Running
    } else {
        StageState::Pending
    };
    let parse_state = if failed(&["parse", "chunk"]) {
        StageState::Error
    } else if chunk.is_some() {
        StageState::Done
    } else if parse.is_some() || (running && input.is_some()) {
        StageState::Running
    } else {
        StageState::Pending
    };
    let embed_state = if skipped {
        StageState::Skipped
    } else if failed(&["embed", "reconcile"]) {
        StageState::Error
    } else if embed_done {
        StageState::Done
    } else if embed_started || (running && chunk.is_some()) {
        StageState::Running
    } else {
        StageState::Pending
    };
    let write_state = if skipped {
        StageState::Skipped
    } else if failed(&["write", "cleanup"]) {
        StageState::Error
    } else if write_events.len() >= 2 || cleanup.is_some() {
        StageState::Done
    } else if !write_events.is_empty() || (running && embed_done) {
        StageState::Running
    } else {
        StageState::Pending
    };
    vec![
        StageView {
            state: input_state,
            detail: input
                .map(|event| event.message.clone())
                .unwrap_or_else(|| "等待提交文档".to_string()),
            duration_ms: input.map(|_| 0),
        },
        StageView {
            state: parse_state,
            detail: chunk
                .or(parse)
                .map(|event| event.message.clone())
                .unwrap_or_else(|| "标题感知切块".to_string()),
            duration_ms: chunk_duration,
        },
        StageView {
            state: embed_state,
            detail: reconcile
                .map(|event| event.message.clone())
                .or_else(|| {
                    job.events
                        .iter()
                        .rev()
                        .find(|event| event.stage == "embed")
                        .map(|event| event.message.clone())
                })
                .unwrap_or_else(|| "等待向量化".to_string()),
            duration_ms: embed_duration,
        },
        StageView {
            state: write_state,
            detail: cleanup
                .map(|event| event.message.clone())
                .or_else(|| write_events.last().map(|event| event.message.clone()))
                .unwrap_or_else(|| "等待写入".to_string()),
            duration_ms: write_duration,
        },
    ]
}

fn render_stage_card(
    number: &str,
    title: &str,
    subtitle: &str,
    icon: Html,
    view: &StageView,
) -> Html {
    let state_icon = match view.state {
        StageState::Pending => html! { <span class="stage-dot"></span> },
        StageState::Running => html! { <Spinner /> },
        StageState::Done => icons::check(),
        StageState::Skipped => html! { <span class="stage-dash">{ "—" }</span> },
        StageState::Error => icons::close(),
    };
    let elapsed = view
        .duration_ms
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    html! {
        <div class={classes!("stage-card", view.state.class())}>
            <div class="stage-card-head">
                <span class="stage-number">{ number }</span>
                <span class="stage-state">
                    { state_icon }
                    { view.state.label() }
                </span>
            </div>
            <span class="stage-icon">{ icon }</span>
            <h3>{ title }</h3>
            <p class="stage-sub">{ subtitle }</p>
            <p class="stage-detail">{ view.detail.clone() }</p>
            <div class="stage-foot">
                <span>{ "耗时" }</span>
                <span class="mono">{ elapsed }</span>
            </div>
        </div>
    }
}

fn chunk_previews_from(job: &PipelineJob) -> Vec<PipelineChunkPreview> {
    if let Some(result) = &job.result {
        if !result.chunk_previews.is_empty() {
            return result.chunk_previews.clone();
        }
    }
    job.events
        .iter()
        .find(|event| event.stage == "chunk")
        .and_then(|event| event.data.get("previews"))
        .and_then(|value| serde_json::from_value(value.clone()).ok())
        .unwrap_or_default()
}

fn vector_preview_from(job: &PipelineJob) -> (Vec<f32>, usize, f32) {
    if let Some(result) = &job.result {
        if !result.vector_preview.is_empty() {
            return (
                result.vector_preview.clone(),
                result.dimension,
                result.vector_norm,
            );
        }
    }
    job.events
        .iter()
        .rev()
        .find(|event| event.stage == "embed" && event.data.get("preview").is_some())
        .map(|event| {
            let values = serde_json::from_value::<Vec<f32>>(
                event.data.get("preview").cloned().unwrap_or(Value::Null),
            )
            .unwrap_or_default();
            let dimension = event
                .data
                .get("dimension")
                .and_then(Value::as_u64)
                .unwrap_or(0) as usize;
            let norm = event
                .data
                .get("norm")
                .and_then(Value::as_f64)
                .unwrap_or(0.0) as f32;
            (values, dimension, norm)
        })
        .unwrap_or_default()
}

fn status_label(status: JobStatus) -> (&'static str, &'static str) {
    match status {
        JobStatus::Queued => ("排队中", "is-queued"),
        JobStatus::Running => ("运行中", "is-running"),
        JobStatus::Succeeded => ("已完成", "is-succeeded"),
        JobStatus::Failed => ("失败", "is-failed"),
    }
}

fn stage_badge(stage: &str) -> (&'static str, &'static str) {
    match stage {
        "input" => ("输入", "is-input"),
        "parse" => ("解析", "is-parse"),
        "chunk" => ("切块", "is-chunk"),
        "reconcile" => ("对账", "is-reconcile"),
        "embed" => ("嵌入", "is-embed"),
        "write" => ("写入", "is-write"),
        "cleanup" => ("清理", "is-cleanup"),
        "done" => ("完成", "is-done"),
        "error" => ("错误", "is-error"),
        _ => ("事件", "is-muted"),
    }
}

fn render_console(
    job: &Option<PipelineJob>,
    error: &Option<String>,
    embedding_configured: bool,
    on_view: Callback<()>,
    on_reset: Callback<()>,
    log_ref: NodeRef,
    submitting: bool,
) -> Html {
    let Some(job) = job.as_ref() else {
        return html! {
            <div class="console-empty">
                <div class="console-orb" aria-hidden="true"><span></span></div>
                <p class="card-kicker">{ "Live pipeline" }</p>
                <h3>{ "等待第一条文档" }</h3>
                <p class="console-empty-text">
                    { if embedding_configured {
                        "提交后，这里会实时打印解析、切块、嵌入与写入事件。"
                    } else {
                        "先在左侧完成模型接入，提交后会在这里看到完整过程。"
                    } }
                </p>
                <div class="console-mini-steps">
                    <span>{ "输入" }</span>
                    <i>{ "→" }</i>
                    <span>{ "切块" }</span>
                    <i>{ "→" }</i>
                    <span>{ "嵌入" }</span>
                    <i>{ "→" }</i>
                    <span class="accent">{ "Qdrant" }</span>
                </div>
                { if let Some(message) = error {
                    html! { <p class="form-message is-error">{ message.clone() }</p> }
                } else { Html::default() } }
            </div>
        };
    };

    let (status_text, status_class) = status_label(job.status);
    let elapsed = job.events.last().map(|event| event.elapsed_ms).unwrap_or(0);
    let chunk_previews = chunk_previews_from(job);
    let (vector_values, vector_dim, vector_norm) = vector_preview_from(job);
    let short_id = job.id.get(..8).unwrap_or(&job.id).to_string();
    let running = !job.status.is_finished();

    html! {
        <div class="console">
            <div class="console-head">
                <div>
                    <p class="card-kicker">{ "Live pipeline" }</p>
                    <h3>{ "运行日志" }</h3>
                </div>
                <span class={classes!("job-status", status_class)}>
                    { if running { html! { <Spinner /> } } else { Html::default() } }
                    { status_text }
                </span>
            </div>
            <div class="console-meta">
                <span class="mono">{ format!("job {}", short_id) }</span>
                <span>{ format!("集合 {}", job.collection) }</span>
                <span>{ format!("耗时 {}", format_duration(elapsed)) }</span>
                { job.doc_id_hint.as_ref().map(|id| html! {
                    <span class="mono">{ format!("doc {}", truncate(id, 18)) }</span>
                }) }
            </div>

            { if let Some(message) = &job.error {
                html! {
                    <div class="console-error">
                        <strong>{ "入库失败" }</strong>
                        <p>{ message.clone() }</p>
                    </div>
                }
            } else if let Some(message) = error {
                html! { <div class="console-error"><p>{ message.clone() }</p></div> }
            } else { Html::default() } }

            { if let Some(result) = &job.result {
                render_result(result, on_view.clone(), !submitting)
            } else { Html::default() } }

            <div class="console-previews">
                <div class="preview-panel">
                    <div class="preview-head">
                        <h4>{ "首条向量" }</h4>
                        <span class="chart-note">
                            { if vector_dim == 0 { "等待".to_string() } else { format!("{vector_dim} 维") } }
                        </span>
                    </div>
                    <VectorBars values={vector_values} dimension={vector_dim} norm={vector_norm} />
                </div>
                <div class="preview-panel">
                    <div class="preview-head">
                        <h4>{ "切块预览" }</h4>
                        <span class="chart-note">{ format!("{} 块", chunk_previews.len()) }</span>
                    </div>
                    { render_chunks(&chunk_previews) }
                </div>
            </div>

            <div class="event-log-wrap">
                <div class="event-log-head">
                    <h4>{ "事件流" }</h4>
                    <div class="event-log-actions">
                        <span class="chart-note">{ format!("{} 条", job.events.len()) }</span>
                        <button class="icon-button icon-button-dark" type="button"
                            title="清空当前调试视图"
                            onclick={Callback::from(move |_| on_reset.emit(()))}
                            disabled={running}>
                            { icons::refresh() }
                        </button>
                    </div>
                </div>
                <ul class="event-log" ref={log_ref}>
                    { for job.events.iter().map(|event| {
                        let (label, class) = stage_badge(&event.stage);
                        html! {
                            <li class="event-row">
                                <span class="event-time mono">{ format_clock(&event.at) }</span>
                                <span class={classes!("event-badge", class)}>{ label }</span>
                                <span class="event-message">{ event.message.clone() }</span>
                                <span class="event-ms mono">{ format!("{} ms", event.elapsed_ms) }</span>
                            </li>
                        }
                    }) }
                </ul>
                { if running {
                    html! {
                        <p class="console-live">
                            <i class="status-dot is-pulse"></i>
                            { "实时接收后端事件…" }
                        </p>
                    }
                } else { Html::default() } }
            </div>
        </div>
    }
}

fn render_result(
    result: &kb_dashboard_types::PipelineResult,
    on_view: Callback<()>,
    allow_reset: bool,
) -> Html {
    let hash = result
        .content_hash
        .get(..12)
        .map(|prefix| format!("{prefix}…"))
        .unwrap_or_else(|| result.content_hash.clone());
    html! {
        <div class="result-card">
            <div class="result-head">
                <div class="result-title">
                    <span class="result-check">{ icons::check() }</span>
                    <div>
                        <h4>{ "入库结果" }</h4>
                        <p>
                            { if result.skipped {
                                "内容未变化，已跳过一次嵌入"
                            } else if result.metadata_updated {
                                "仅元数据变化，向量保持不变"
                            } else {
                                "文档已写入并可被检索"
                            } }
                        </p>
                    </div>
                </div>
                <button class="button button-small button-ghost-light" type="button"
                    onclick={Callback::from(move |_| on_view.emit(()))} disabled={!allow_reset}>
                    { "在浏览器中查看" }{ icons::chevron_right() }
                </button>
            </div>
            <div class="result-grid">
                <div class="result-fact">
                    <span class="result-label">{ "文档 ID" }</span>
                    <span class="result-value mono">{ result.doc_id.clone() }</span>
                </div>
                <div class="result-fact">
                    <span class="result-label">{ "块数" }</span>
                    <span class="result-value">{ result.chunks.to_string() }</span>
                </div>
                <div class="result-fact">
                    <span class="result-label">{ "向量点" }</span>
                    <span class="result-value mono">
                        { format!("{} → {}", format_int(result.points_before), format_int(result.points_after)) }
                    </span>
                </div>
                <div class="result-fact">
                    <span class="result-label">{ "嵌入模型" }</span>
                    <span class="result-value mono">{ result.embedding_model.clone() }</span>
                </div>
                <div class="result-fact">
                    <span class="result-label">{ "维度" }</span>
                    <span class="result-value">{ result.dimension.to_string() }</span>
                </div>
                <div class="result-fact">
                    <span class="result-label">{ "耗时" }</span>
                    <span class="result-value">{ format_duration(result.duration_ms) }</span>
                </div>
                <div class="result-fact result-fact-wide">
                    <span class="result-label">{ "内容哈希" }</span>
                    <span class="result-value mono">{ hash }</span>
                </div>
            </div>
            { if result.skipped || result.metadata_updated {
                html! {
                    <p class="result-note">
                        { icons::check() }
                        { if result.skipped {
                            "再次提交相同内容会命中幂等对账：不调用嵌入 API，也不重写向量。"
                        } else {
                            "文本未变，仅更新了标题、来源或标签等元数据。"
                        } }
                    </p>
                }
            } else { Html::default() } }
        </div>
    }
}

fn render_chunks(previews: &[PipelineChunkPreview]) -> Html {
    if previews.is_empty() {
        return html! {
            <div class="preview-empty">
                <span>{ "切块后，这里会显示每块的开头。" }</span>
            </div>
        };
    }
    html! {
        <ul class="chunk-list">
            { for previews.iter().map(|preview| html! {
                <li class="chunk-item">
                    <span class="chunk-index">{ format!("{:02}", preview.index + 1) }</span>
                    <div class="chunk-body">
                        <p class="chunk-heading">
                            { if preview.heading_path.is_empty() {
                                "无标题路径".to_string()
                            } else {
                                preview.heading_path.clone()
                            } }
                        </p>
                        <p class="chunk-text">{ preview.preview.clone() }</p>
                    </div>
                    <span class="chunk-chars">{ format!("{} 字", preview.characters) }</span>
                </li>
            }) }
        </ul>
    }
}
