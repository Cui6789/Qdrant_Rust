use gloo_timers::callback::Interval;
use kb_dashboard_types::{
    CollectionSummary, DocumentDetailResponse, DocumentListResponse, SearchResponse,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::HtmlInputElement;
use yew::prelude::*;

use crate::api;
use crate::components::ui::{EmptyState, SectionHeading, Spinner};
use crate::icons;
use crate::util::{format_int, format_time, truncate};

#[derive(Properties, PartialEq)]
pub struct BrowserProps {
    pub collections: Vec<CollectionSummary>,
    pub active_collection: Option<String>,
    pub on_collection: Callback<String>,
    pub refresh_nonce: u32,
    pub embedding_configured: bool,
    pub on_changed: Callback<()>,
}

#[function_component(BrowserSection)]
pub fn browser_section(props: &BrowserProps) -> Html {
    let docs = use_state(|| None::<DocumentListResponse>);
    let loading = use_state(|| false);
    let error = use_state(|| None::<String>);
    let filter_input = use_state(String::new);
    let applied_filter = use_state(String::new);
    let live = use_state(|| true);
    let manual_tick = use_state(|| 0u32);
    let detail = use_state(|| None::<DocumentDetailResponse>);
    let detail_loading = use_state(|| false);
    let detail_error = use_state(|| None::<String>);
    let deleting = use_state(|| false);
    let search_query = use_state(String::new);
    let searching = use_state(|| false);
    let search_error = use_state(|| None::<String>);
    let search_results = use_state(|| None::<SearchResponse>);

    {
        let on_collection = props.on_collection.clone();
        let active = props.active_collection.clone();
        let first = props.collections.first().map(|entry| entry.name.clone());
        use_effect_with((active, first), move |(active, first)| {
            if active.is_none() {
                if let Some(name) = first {
                    on_collection.emit(name.clone());
                }
            }
            || ()
        });
    }

    {
        let search_results = search_results.clone();
        let detail = detail.clone();
        let detail_error = detail_error.clone();
        let active = props.active_collection.clone();
        use_effect_with(active, move |_| {
            search_results.set(None);
            detail.set(None);
            detail_error.set(None);
            || ()
        });
    }

    {
        let docs = docs.clone();
        let loading = loading.clone();
        let error = error.clone();
        let active = props.active_collection.clone();
        let filter = (*applied_filter).clone();
        use_effect_with((active, filter), move |_| {
            docs.set(None);
            loading.set(true);
            error.set(None);
            || ()
        });
    }

    {
        let docs = docs.clone();
        let error = error.clone();
        let loading = loading.clone();
        let collection = props.active_collection.clone();
        let filter = (*applied_filter).clone();
        let nonce = props.refresh_nonce;
        let live = *live;
        let tick = *manual_tick;
        use_effect_with(
            (collection.clone(), filter.clone(), nonce, live, tick),
            move |_| {
                let has_collection = collection.is_some();
                let fetch = {
                    let docs = docs.clone();
                    let error = error.clone();
                    let loading = loading.clone();
                    let collection = collection.clone();
                    let filter = filter.clone();
                    move || {
                        let Some(name) = collection.clone() else {
                            docs.set(None);
                            error.set(None);
                            return;
                        };
                        let docs = docs.clone();
                        let error = error.clone();
                        let loading = loading.clone();
                        if docs.is_none() {
                            loading.set(true);
                        }
                        let query = filter.clone();
                        spawn_local(async move {
                            let query_opt = (!query.trim().is_empty()).then_some(query.as_str());
                            match api::documents(&name, query_opt, 2_000).await {
                                Ok(data) => {
                                    docs.set(Some(data));
                                    error.set(None);
                                }
                                Err(failure) => error.set(Some(failure.message)),
                            }
                            loading.set(false);
                        });
                    }
                };
                fetch();
                let interval = if live && has_collection {
                    Some(Interval::new(3_500, fetch))
                } else {
                    None
                };
                move || drop(interval)
            },
        );
    }

    let open_detail = {
        let detail = detail.clone();
        let detail_loading = detail_loading.clone();
        let detail_error = detail_error.clone();
        let collection = props.active_collection.clone();
        Callback::from(move |doc_id: String| {
            let Some(collection) = collection.clone() else {
                return;
            };
            detail.set(None);
            detail_loading.set(true);
            detail_error.set(None);
            let detail = detail.clone();
            let detail_loading = detail_loading.clone();
            let detail_error = detail_error.clone();
            spawn_local(async move {
                match api::document_detail(&collection, &doc_id).await {
                    Ok(data) => detail.set(Some(data)),
                    Err(failure) => detail_error.set(Some(failure.message)),
                }
                detail_loading.set(false);
            });
        })
    };
    let close_detail = {
        let detail = detail.clone();
        let detail_loading = detail_loading.clone();
        let detail_error = detail_error.clone();
        Callback::from(move |_| {
            detail.set(None);
            detail_loading.set(false);
            detail_error.set(None);
        })
    };

    let on_delete = {
        let detail = detail.clone();
        let collection = props.active_collection.clone();
        let deleting = deleting.clone();
        let detail_error = detail_error.clone();
        let manual_tick = manual_tick.clone();
        let on_changed = props.on_changed.clone();
        let search_results = search_results.clone();
        Callback::from(move |_| {
            let Some(current) = (*detail).clone() else {
                return;
            };
            let Some(collection) = collection.clone() else {
                return;
            };
            let confirmed = web_sys::window()
                .and_then(|window| {
                    window
                        .confirm_with_message(&format!(
                            "删除文档「{}」的全部 {} 个块？此操作不可撤销。",
                            current.doc_id, current.chunk_count
                        ))
                        .ok()
                })
                .unwrap_or(false);
            if !confirmed {
                return;
            }
            deleting.set(true);
            detail_error.set(None);
            let detail = detail.clone();
            let deleting = deleting.clone();
            let detail_error = detail_error.clone();
            let manual_tick = manual_tick.clone();
            let on_changed = on_changed.clone();
            let search_results = search_results.clone();
            spawn_local(async move {
                match api::delete_document(&collection, &current.doc_id).await {
                    Ok(_) => {
                        detail.set(None);
                        search_results.set(None);
                        manual_tick.set(*manual_tick + 1);
                        on_changed.emit(());
                    }
                    Err(failure) => detail_error.set(Some(failure.message)),
                }
                deleting.set(false);
            });
        })
    };

    let on_filter_submit = {
        let applied_filter = applied_filter.clone();
        let filter_input = filter_input.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            applied_filter.set(filter_input.trim().to_string());
        })
    };
    let on_filter_input = {
        let filter_input = filter_input.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            filter_input.set(input.value());
        })
    };
    let on_filter_clear = {
        let filter_input = filter_input.clone();
        let applied_filter = applied_filter.clone();
        Callback::from(move |_| {
            filter_input.set(String::new());
            applied_filter.set(String::new());
        })
    };
    let on_refresh = {
        let manual_tick = manual_tick.clone();
        Callback::from(move |_| manual_tick.set(*manual_tick + 1))
    };
    let on_toggle_live = {
        let live = live.clone();
        Callback::from(move |_| live.set(!*live))
    };
    let on_search_input = {
        let search_query = search_query.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            search_query.set(input.value());
        })
    };
    let on_search = {
        let search_query = search_query.clone();
        let searching = searching.clone();
        let search_error = search_error.clone();
        let search_results = search_results.clone();
        let collection = props.active_collection.clone();
        let embedding_configured = props.embedding_configured;
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            if *searching {
                return;
            }
            let query = search_query.trim().to_string();
            if query.is_empty() {
                search_error.set(Some("输入一句话再检索".to_string()));
                return;
            }
            if !embedding_configured {
                search_error.set(Some("请先在「模型接入」中配置嵌入服务".to_string()));
                return;
            }
            let Some(collection) = collection.clone() else {
                search_error.set(Some("请先选择一个集合".to_string()));
                return;
            };
            searching.set(true);
            search_error.set(None);
            search_results.set(None);
            let searching = searching.clone();
            let search_error = search_error.clone();
            let search_results = search_results.clone();
            spawn_local(async move {
                match api::search(&collection, &query, 8).await {
                    Ok(response) => search_results.set(Some(response)),
                    Err(failure) => search_error.set(Some(failure.message)),
                }
                searching.set(false);
            });
        })
    };
    let on_search_clear = {
        let search_results = search_results.clone();
        let search_error = search_error.clone();
        Callback::from(move |_| {
            search_results.set(None);
            search_error.set(None);
        })
    };

    let active = props.active_collection.clone();
    let document_list = docs
        .as_ref()
        .map(|data| data.documents.clone())
        .unwrap_or_default();
    let document_count = document_list.len();
    let scanned_points = docs.as_ref().map(|data| data.scanned_points).unwrap_or(0);
    let points_count = docs.as_ref().map(|data| data.points_count).unwrap_or(0);
    let vector_size = docs.as_ref().map(|data| data.vector_size).unwrap_or(0);
    let partial = docs.as_ref().map(|data| data.partial).unwrap_or(false);
    let limit = docs.as_ref().map(|data| data.limit).unwrap_or(0);
    let scanned_at = docs.as_ref().map(|data| data.scanned_at.clone());
    let live_on = *live;

    html! {
        <section id="browser" class="section browser-section">
            <div class="container">
                <SectionHeading
                    eyebrow={"Data Browser".to_string()}
                    title={html! { <>{"数据库里的，"}<span class="serif-accent">{"每一篇"}</span>{"。"}</> }}
                    subtitle={html! { "按文档聚合 Qdrant 中的真实 payload：块数、字符、标签、模型与写入时间一目了然。" }}
                />

                <div class="browser-toolbar reveal">
                    <div class="browser-live">
                        <span class={classes!("live-pill", live_on.then_some("is-live"))}>
                            <i class="status-dot"></i>
                            { if live_on { "自动刷新" } else { "已暂停" } }
                        </span>
                        { if let Some(time) = &scanned_at {
                            html! { <span class="browser-time">{ format!("最近读取 {}", format_time(time)) }</span> }
                        } else { Html::default() } }
                    </div>
                    <div class="browser-actions">
                        <button class={classes!("toggle", live_on.then_some("is-on"))}
                            type="button" onclick={on_toggle_live} aria-pressed={live_on.to_string()}>
                            <span class="toggle-knob"></span>
                            { if live_on { "实时" } else { "暂停" } }
                        </button>
                        <button class="icon-button" title="立即刷新" onclick={on_refresh.clone()}>
                            { icons::refresh() }
                        </button>
                    </div>
                </div>

                { if props.collections.is_empty() {
                    html! {
                        <div class="notice is-warning">
                            <span class="notice-icon">{ icons::layers() }</span>
                            <div>
                                <strong>{ "还没有集合" }</strong>
                                <p>{ "在上方入库流程中提交第一篇文档，集合会自动创建。" }</p>
                            </div>
                            <a class="button button-small" href="#pipeline">{ "去入库" }</a>
                        </div>
                    }
                } else {
                    html! {
                        <div class="collection-tabs reveal">
                            { for props.collections.iter().map(|entry| {
                                let name = entry.name.clone();
                                let target = name.clone();
                                let on_collection = props.on_collection.clone();
                                let is_active = active.as_deref() == Some(name.as_str());
                                html! {
                                    <button type="button" class={classes!("collection-tab", is_active.then_some("is-active"))}
                                        onclick={Callback::from(move |_| on_collection.emit(target.clone()))}>
                                        <span class="collection-tab-name">{ entry.name.clone() }</span>
                                        <span class="collection-tab-meta">
                                            { format!("{} 点 · {} 维", format_int(entry.points_count), entry.vector_size) }
                                        </span>
                                    </button>
                                }
                            }) }
                        </div>
                    }
                } }

                <div class="browser-filter-bar reveal">
                    <form class="inline-form" onsubmit={on_filter_submit}>
                        <span class="input-with-icon">
                            { icons::search() }
                            <input
                                class="input input-search"
                                type="text"
                                placeholder="按文档 ID、标题、来源或标签过滤"
                                value={(*filter_input).clone()}
                                oninput={on_filter_input}
                            />
                        </span>
                        <button class="button button-small button-ghost" type="submit">{ "过滤" }</button>
                        { if !applied_filter.is_empty() {
                            html! { <button class="button button-small button-text" type="button" onclick={on_filter_clear}>{ "清除" }</button> }
                        } else { Html::default() } }
                    </form>
                    <form class="inline-form search-form" onsubmit={on_search}>
                        <span class="input-with-icon">
                            { icons::spark() }
                            <input
                                class="input input-search"
                                type="text"
                                placeholder="语义检索：例如「年假怎么申请」"
                                value={(*search_query).clone()}
                                oninput={on_search_input}
                            />
                        </span>
                        <button class="button button-small button-primary" type="submit"
                            disabled={*searching || !props.embedding_configured}>
                            { if *searching { html! { <Spinner /> } } else { icons::search() } }
                            { "检索" }
                        </button>
                    </form>
                </div>

                { if !props.embedding_configured {
                    html! {
                        <p class="inline-hint">
                            { icons::key() }
                            { "语义检索需要嵌入模型；当前仅能浏览与过滤文档。" }
                            <a href="#config">{ "去接入" }</a>
                        </p>
                    }
                } else { Html::default() } }

                { if let Some(failure) = &*search_error {
                    html! { <p class="form-message is-error">{ failure.clone() }</p> }
                } else { Html::default() } }

                { if let Some(response) = &*search_results {
                    html! {
                        <div class="search-panel reveal">
                            <div class="search-panel-head">
                                <div>
                                    <p class="card-kicker">{ "Semantic Search" }</p>
                                    <h3>{ format!("「{}」的最近邻", response.query) }</h3>
                                </div>
                                <div class="search-panel-meta">
                                    <span class="chip chip-muted">{ format!("{} 维", vector_size) }</span>
                                    <span class="chip chip-muted">{ format!("{} ms", response.took_ms) }</span>
                                    <button class="icon-button" type="button" onclick={on_search_clear} title="关闭检索结果">
                                        { icons::close() }
                                    </button>
                                </div>
                            </div>
                            { if response.results.is_empty() {
                                html! { <div class="preview-empty"><span>{ "没有命中的块。" }</span></div> }
                            } else {
                                html! {
                                    <ul class="search-results">
                                        { for response.results.iter().map(|hit| {
                                            let doc_id = hit.doc_id.clone();
                                            let open = open_detail.clone();
                                            let width = hit.score.clamp(0.0, 1.0) * 100.0;
                                            html! {
                                                <li>
                                                    <button type="button" class="search-hit"
                                                        onclick={Callback::from(move |_| open.emit(doc_id.clone()))}>
                                                        <span class="search-score">
                                                            <span class="search-score-value mono">{ format!("{:.3}", hit.score) }</span>
                                                            <span class="score-track"><span class="score-fill" style={format!("width:{width:.1}%")}></span></span>
                                                        </span>
                                                        <span class="search-hit-body">
                                                            <span class="search-hit-title">
                                                                { hit.title.clone().unwrap_or_else(|| hit.doc_id.clone()) }
                                                                <span class="search-hit-doc mono">{ hit.doc_id.clone() }</span>
                                                            </span>
                                                            <span class="search-hit-text">{ truncate(&hit.text, 180) }</span>
                                                            { if !hit.heading_path.is_empty() {
                                                                html! { <span class="search-hit-heading">{ hit.heading_path.clone() }</span> }
                                                            } else { Html::default() } }
                                                        </span>
                                                        { icons::chevron_right() }
                                                    </button>
                                                </li>
                                            }
                                        }) }
                                    </ul>
                                }
                            } }
                        </div>
                    }
                } else { Html::default() } }

                { if let Some(failure) = &*error {
                    html! {
                        <div class="notice is-error">
                            <span class="notice-icon">{ icons::activity() }</span>
                            <div>
                                <strong>{ "读取文档失败" }</strong>
                                <p>{ failure.clone() }</p>
                            </div>
                            <button class="button button-small" onclick={on_refresh.clone()}>{ "重试" }</button>
                        </div>
                    }
                } else { Html::default() } }

                { if partial && document_count > 0 {
                    html! {
                        <div class="notice is-info compact">
                            <span class="notice-icon">{ icons::activity() }</span>
                            <div>
                                <strong>{ "当前为部分扫描" }</strong>
                                <p>{ format!("已扫描 {} 个块（本次上限 {}，集合共 {} 个点）。大集合可先按关键字过滤，或直接打开文档。", format_int(scanned_points), limit, format_int(points_count)) }</p>
                            </div>
                        </div>
                    }
                } else { Html::default() } }

                <div class="browser-summary reveal">
                    <span><strong>{ document_count }</strong>{" 篇文档"}</span>
                    <span><strong>{ format_int(points_count) }</strong>{" 个向量点"}</span>
                    <span><strong>{ format_int(vector_size) }</strong>{" 维"}</span>
                    <span><strong>{ format_int(scanned_points) }</strong>{" 块已扫描"}</span>
                </div>

                { if *loading && docs.is_none() {
                    html! {
                        <div class="doc-skeleton">
                            { for (0..4).map(|_| html! {
                                <div class="skeleton-row">
                                    <span class="skeleton-bar"></span>
                                    <span class="skeleton-bar short"></span>
                                    <span class="skeleton-bar tiny"></span>
                                </div>
                            }) }
                        </div>
                    }
                } else if document_list.is_empty() {
                    html! {
                        <EmptyState
                            icon={icons::database()}
                            title={if active.is_some() { "这个集合还没有文档".to_string() } else { "选择一个集合开始浏览".to_string() }}
                            text={if applied_filter.is_empty() {
                                "去上方入库流程写入第一篇，或切换到其他集合。".to_string()
                            } else {
                                format!("没有匹配「{}」的文档，试试清除过滤条件。", *applied_filter)
                            }}
                        />
                    }
                } else {
                    html! {
                        <div class="doc-table reveal">
                            <div class="doc-table-head">
                                <span>{ "文档" }</span>
                                <span>{ "标签" }</span>
                                <span>{ "块" }</span>
                                <span>{ "字符" }</span>
                                <span>{ "模型" }</span>
                                <span>{ "写入时间" }</span>
                                <span></span>
                            </div>
                            { for document_list.iter().map(|document| {
                                let doc_id = document.doc_id.clone();
                                let open = open_detail.clone();
                                let tags = if document.tags.is_empty() {
                                    html! { <span class="muted">{ "—" }</span> }
                                } else {
                                    html! {
                                        <span class="tag-list">
                                            { for document.tags.iter().take(3).map(|tag| html! {
                                                <span class="tag">{ tag.clone() }</span>
                                            }) }
                                            { if document.tags.len() > 3 {
                                                html! { <span class="tag tag-more">{ format!("+{}", document.tags.len() - 3) }</span> }
                                            } else { Html::default() } }
                                        </span>
                                    }
                                };
                                html! {
                                    <button type="button" class="doc-row" key={document.doc_id.clone()}
                                        onclick={Callback::from(move |_| open.emit(doc_id.clone()))}>
                                        <span class="doc-main">
                                            <span class="doc-title">
                                                { document.title.clone().unwrap_or_else(|| document.doc_id.clone()) }
                                            </span>
                                            <span class="doc-sub">
                                                <span class="mono">{ document.doc_id.clone() }</span>
                                                { document.source.as_ref().map(|source| html! {
                                                    <span class="doc-source">{ format!("· {}", source) }</span>
                                                }) }
                                            </span>
                                        </span>
                                        <span class="doc-cell">{ tags }</span>
                                        <span class="doc-cell mono">
                                            { format_int(document.chunk_count) }
                                            { if document.scanned_chunks < document.chunk_count {
                                                html! { <span class="doc-partial" title="当前扫描到部分块">{ format!("/{}", document.scanned_chunks) }</span> }
                                            } else { Html::default() } }
                                        </span>
                                        <span class="doc-cell">{ format_int(document.characters) }</span>
                                        <span class="doc-cell doc-model">{ document.embedding_model.clone().unwrap_or_else(|| "—".to_string()) }</span>
                                        <span class="doc-cell doc-time">
                                            { document.created_at.as_deref().map(format_time).unwrap_or_else(|| "—".to_string()) }
                                        </span>
                                        <span class="doc-chevron">{ icons::chevron_right() }</span>
                                    </button>
                                }
                            }) }
                        </div>
                    }
                } }

                { if *detail_loading || detail.is_some() || detail_error.is_some() {
                    html! {
                        <div class="modal-backdrop" onclick={close_detail.clone()}>
                            <div class="modal" onclick={Callback::from(|event: web_sys::MouseEvent| event.stop_propagation())}>
                                <button class="modal-close" type="button" onclick={close_detail.clone()} aria-label="关闭">
                                    { icons::close() }
                                </button>
                                { if *detail_loading {
                                    html! {
                                        <div class="modal-loading">
                                            <Spinner />
                                            <span>{ "正在读取文档块…" }</span>
                                        </div>
                                    }
                                } else if let Some(document) = &*detail {
                                    html! {
                                        <>
                                            <div class="modal-head">
                                                <p class="card-kicker">{ "Document" }</p>
                                                <h3>{ document.title.clone().unwrap_or_else(|| document.doc_id.clone()) }</h3>
                                                <p class="modal-sub">
                                                    <span class="mono">{ document.doc_id.clone() }</span>
                                                    { document.source.as_ref().map(|source| html! {
                                                        <span>{ format!("· {}", source) }</span>
                                                    }) }
                                                </p>
                                                <div class="modal-chips">
                                                    <span class="chip chip-muted">{ format!("{} 块", document.chunk_count) }</span>
                                                    <span class="chip chip-muted">{ format!("{} 字符", format_int(document.characters)) }</span>
                                                    <span class="chip chip-muted">{ document.embedding_model.clone().unwrap_or_else(|| "—".to_string()) }</span>
                                                    { for document.tags.iter().map(|tag| html! {
                                                        <span class="chip chip-accent">{ tag.clone() }</span>
                                                    }) }
                                                </div>
                                            </div>
                                            { if let Some(failure) = &*detail_error {
                                                html! { <p class="form-message is-error">{ failure.clone() }</p> }
                                            } else { Html::default() } }
                                            <div class="modal-chunks">
                                                { for document.chunks.iter().map(|chunk| html! {
                                                    <div class="modal-chunk">
                                                        <div class="modal-chunk-head">
                                                            <span class="chunk-badge mono">{ format!("{:02}", chunk.chunk_index + 1) }</span>
                                                            <span class="modal-chunk-heading">
                                                                { if chunk.heading_path.is_empty() {
                                                                    "无标题路径".to_string()
                                                                } else {
                                                                    chunk.heading_path.clone()
                                                                } }
                                                            </span>
                                                            <span class="modal-chunk-meta">
                                                                { format!("{} 字", chunk.characters) }
                                                            </span>
                                                        </div>
                                                        <p class="modal-chunk-text">{ chunk.text.clone() }</p>
                                                        <div class="modal-chunk-foot">
                                                            <span class="mono">{ format!("point {}", truncate(&chunk.point_id, 12)) }</span>
                                                            { chunk.created_at.as_deref().map(|time| html! {
                                                                <span>{ format_time(time) }</span>
                                                            }) }
                                                        </div>
                                                    </div>
                                                }) }
                                            </div>
                                            <div class="modal-actions">
                                                <button class="button button-danger" type="button"
                                                    onclick={on_delete.clone()} disabled={*deleting}>
                                                    { if *deleting { html! { <Spinner /> } } else { icons::trash() } }
                                                    { if *deleting { "正在删除…" } else { "删除这篇文档" } }
                                                </button>
                                                <button class="button button-ghost" type="button" onclick={close_detail.clone()}>
                                                    { "关闭" }
                                                </button>
                                            </div>
                                        </>
                                    }
                                } else {
                                    html! {
                                        <div class="modal-loading">
                                            <p class="form-message is-error">
                                                { (*detail_error).clone().unwrap_or_else(|| "读取失败".to_string()) }
                                            </p>
                                            <button class="button button-ghost" type="button" onclick={close_detail.clone()}>{ "关闭" }</button>
                                        </div>
                                    }
                                } }
                            </div>
                        </div>
                    }
                } else { Html::default() } }
            </div>
        </section>
    }
}
