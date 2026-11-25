use kb_dashboard_types::{
    EmbeddingConfigRequest, EmbeddingConfigResponse, EmbeddingConfigStateResponse,
};
use wasm_bindgen_futures::spawn_local;
use web_sys::{HtmlInputElement, HtmlSelectElement};
use yew::prelude::*;

use crate::api;
use crate::components::ui::{Spinner, VectorBars};
use crate::icons;
use crate::util::{format_time, host_label};

// Dimensions are intentionally left empty: the backend probes the real
// dimensionality and avoids sending an unsupported `dimensions` parameter.
// The last field is the wire protocol: `openai` or `dashscope`.
const PRESETS: [(&str, &str, &str, &str, &str); 6] = [
    (
        "OpenAI",
        "https://api.openai.com/v1",
        "text-embedding-3-small",
        "",
        "openai",
    ),
    (
        "通义 MaaS",
        "https://maas.qianwenaiapi.com/compatible-mode/v1",
        "qwen3.7-text-embedding-flash",
        "",
        "openai",
    ),
    (
        "通义原生",
        "https://cn-beijing.maas.qianwenaiapi.com/api/v1",
        "qwen3.7-text-embedding-flash",
        "",
        "dashscope",
    ),
    (
        "百炼兼容",
        "https://dashscope.aliyuncs.com/compatible-mode/v1",
        "text-embedding-v4",
        "",
        "openai",
    ),
    (
        "SiliconFlow",
        "https://api.siliconflow.cn/v1",
        "BAAI/bge-m3",
        "",
        "openai",
    ),
    (
        "Ollama",
        "http://localhost:11434/v1",
        "nomic-embed-text",
        "",
        "openai",
    ),
];

fn protocol_label(protocol: Option<&str>) -> &'static str {
    match protocol {
        Some("dashscope") => "DashScope 原生",
        Some("openai") => "OpenAI 兼容",
        Some("mock") => "本地演示",
        _ => "接口",
    }
}

#[derive(Properties, PartialEq)]
pub struct EmbeddingProps {
    pub on_saved: Callback<()>,
}

#[function_component(EmbeddingSection)]
pub fn embedding_section(props: &EmbeddingProps) -> Html {
    let config_state = use_state(|| None::<EmbeddingConfigStateResponse>);
    let base_url = use_state(String::new);
    let api_key = use_state(String::new);
    let model = use_state(String::new);
    let dimensions = use_state(String::new);
    let protocol = use_state(|| "openai".to_string());
    let show_key = use_state(|| false);
    let submitting = use_state(|| false);
    let error = use_state(|| None::<String>);
    let note = use_state(|| None::<String>);
    let probe = use_state(|| None::<EmbeddingConfigResponse>);

    {
        let config_state = config_state.clone();
        let base_url = base_url.clone();
        let model = model.clone();
        let dimensions = dimensions.clone();
        let protocol = protocol.clone();
        let error = error.clone();
        use_effect_with((), move |_| {
            spawn_local(async move {
                match api::embedding_state().await {
                    Ok(data) => {
                        if data.status.configured && data.status.kind.as_deref() != Some("mock") {
                            if let Some(url) = data.status.base_url.clone() {
                                base_url.set(url);
                            }
                            if let Some(name) = data.status.model.clone() {
                                model.set(name);
                            }
                            // Leave dimensions empty so the next save auto-probes
                            // instead of sending an unsupported parameter.
                            protocol.set(
                                data.status
                                    .protocol
                                    .clone()
                                    .unwrap_or_else(|| "openai".to_string()),
                            );
                        } else {
                            base_url.set(data.defaults.base_url.clone());
                            model.set(data.defaults.model.clone());
                            if data.defaults.dimensions > 0 {
                                dimensions.set(data.defaults.dimensions.to_string());
                            }
                            protocol.set("openai".to_string());
                        }
                        config_state.set(Some(data));
                    }
                    Err(failure) => error.set(Some(failure.message)),
                }
            });
            || ()
        });
    }

    let on_submit = {
        let base_url = base_url.clone();
        let api_key = api_key.clone();
        let model = model.clone();
        let dimensions = dimensions.clone();
        let protocol = protocol.clone();
        let submitting = submitting.clone();
        let error = error.clone();
        let note = note.clone();
        let probe = probe.clone();
        let config_state = config_state.clone();
        let on_saved = props.on_saved.clone();
        Callback::from(move |event: SubmitEvent| {
            event.prevent_default();
            if *submitting {
                return;
            }
            let base = base_url.trim().to_string();
            let key = api_key.trim().to_string();
            let name = model.trim().to_string();
            if !(base.starts_with("http://") || base.starts_with("https://")) {
                error.set(Some("Base URL 需要以 http:// 或 https:// 开头".to_string()));
                return;
            }
            if name.is_empty() {
                error.set(Some("请填写模型名称".to_string()));
                return;
            }
            let dims = match dimensions.trim() {
                "" => None,
                value => match value.parse::<usize>() {
                    Ok(0) => None,
                    Ok(value) => Some(value),
                    Err(_) => {
                        error.set(Some("维度必须是正整数，或留空自动探测".to_string()));
                        return;
                    }
                },
            };
            let request = EmbeddingConfigRequest {
                base_url: base,
                api_key: key,
                model: name,
                dimensions: dims,
                protocol: Some((*protocol).clone()),
            };
            submitting.set(true);
            error.set(None);
            note.set(None);
            probe.set(None);
            let submitting = submitting.clone();
            let error = error.clone();
            let note = note.clone();
            let probe = probe.clone();
            let api_key_field = api_key.clone();
            let config_state = config_state.clone();
            let on_saved = on_saved.clone();
            spawn_local(async move {
                match api::configure_embedding(&request).await {
                    Ok(response) => {
                        let latency = response.latency_ms;
                        let dimension = response.probe.dimension;
                        if let Some(mut state) = (*config_state).clone() {
                            state.status = response.status.clone();
                            config_state.set(Some(state));
                        }
                        // Do not keep the secret in the DOM after a successful
                        // save; the process still holds it in memory.
                        api_key_field.set(String::new());
                        note.set(Some(format!(
                            "连接成功：{dimension} 维 · 探针耗时 {latency} ms"
                        )));
                        probe.set(Some(response));
                        on_saved.emit(());
                    }
                    Err(failure) => error.set(Some(failure.message)),
                }
                submitting.set(false);
            });
        })
    };

    let on_mock = {
        let submitting = submitting.clone();
        let error = error.clone();
        let note = note.clone();
        let probe = probe.clone();
        let config_state = config_state.clone();
        let on_saved = props.on_saved.clone();
        Callback::from(move |_| {
            if *submitting {
                return;
            }
            submitting.set(true);
            error.set(None);
            note.set(None);
            let submitting = submitting.clone();
            let error = error.clone();
            let note = note.clone();
            let probe = probe.clone();
            let config_state = config_state.clone();
            let on_saved = on_saved.clone();
            spawn_local(async move {
                match api::configure_mock().await {
                    Ok(status) => {
                        if let Some(mut state) = (*config_state).clone() {
                            state.status = status;
                            config_state.set(Some(state));
                        }
                        probe.set(None);
                        note.set(Some(
                            "已启用本地演示嵌入：确定性向量，不调用外部 API。".to_string(),
                        ));
                        on_saved.emit(());
                    }
                    Err(failure) => error.set(Some(failure.message)),
                }
                submitting.set(false);
            });
        })
    };

    let on_clear = {
        let error = error.clone();
        let note = note.clone();
        let probe = probe.clone();
        let config_state = config_state.clone();
        let on_saved = props.on_saved.clone();
        Callback::from(move |_| {
            let error = error.clone();
            let note = note.clone();
            let probe = probe.clone();
            let config_state = config_state.clone();
            let on_saved = on_saved.clone();
            spawn_local(async move {
                match api::clear_embedding().await {
                    Ok(status) => {
                        if let Some(mut state) = (*config_state).clone() {
                            state.status = status;
                            config_state.set(Some(state));
                        }
                        probe.set(None);
                        note.set(Some("已清除内存中的嵌入配置与密钥。".to_string()));
                        on_saved.emit(());
                    }
                    Err(failure) => error.set(Some(failure.message)),
                }
            });
        })
    };

    let status = config_state.as_ref().map(|state| state.status.clone());
    let configured = status
        .as_ref()
        .map(|status| status.configured)
        .unwrap_or(false);
    let is_mock = status.as_ref().and_then(|status| status.kind.as_deref()) == Some("mock");

    let on_base_input = {
        let base_url = base_url.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            base_url.set(input.value());
        })
    };
    let on_key_input = {
        let api_key = api_key.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            api_key.set(input.value());
        })
    };
    let on_model_input = {
        let model = model.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            model.set(input.value());
        })
    };
    let on_dims_input = {
        let dimensions = dimensions.clone();
        Callback::from(move |event: InputEvent| {
            let input: HtmlInputElement = event.target_unchecked_into();
            dimensions.set(input.value());
        })
    };
    let on_protocol_change = {
        let protocol = protocol.clone();
        Callback::from(move |event: web_sys::Event| {
            let select: HtmlSelectElement = event.target_unchecked_into();
            protocol.set(select.value());
        })
    };
    let toggle_key = {
        let show_key = show_key.clone();
        Callback::from(move |_| show_key.set(!*show_key))
    };

    html! {
        <section id="config" class="section config-section">
            <div class="container">
                <div class="config-shell reveal">
                    <div class="config-copy">
                        <p class="eyebrow">{ "Embedding API" }</p>
                        <h2>{ "把模型，" }<span class="serif-accent">{ "接进来。" }</span></h2>
                        <p class="config-lede">
                            { "填写 OpenAI 兼容的 /embeddings 端点，或通义 MaaS 的原生 TextEmbedding 接口。密钥只保存在当前进程的内存里，不写盘、不写日志。" }
                        </p>
                        <ul class="feature-list">
                            <li>{ icons::check() }{ "OpenAI 兼容 + DashScope 原生" }</li>
                            <li>{ icons::check() }{ "自动探测真实向量维度" }</li>
                            <li>{ icons::check() }{ "实时探针：看到第一条真实向量" }</li>
                            <li>{ icons::check() }{ "无 API？可切换本地确定性演示嵌入" }</li>
                        </ul>
                        <div class={classes!("config-status", configured.then_some("is-ready"))}>
                            <span class="config-status-icon">
                                { if configured { icons::check() } else { icons::key() } }
                            </span>
                            <div>
                                <p class="config-status-title">
                                    { if configured { "嵌入服务已就绪" } else { "尚未接入嵌入服务" } }
                                </p>
                                <p class="config-status-text">
                                    { match &status {
                                        Some(status) if status.configured => {
                                            format!(
                                                "{} · {} 维 · {} · {}",
                                                status.model.clone().unwrap_or_else(|| "—".to_string()),
                                                status.dimension.unwrap_or(0),
                                                status.base_url.as_deref().map(host_label).unwrap_or_else(|| "—".to_string()),
                                                protocol_label(status.protocol.as_deref())
                                            )
                                        }
                                        _ => "配置后即可运行入库流程与相似度检索。".to_string(),
                                    } }
                                </p>
                                { if is_mock {
                                    html! { <span class="chip chip-warning">{ "演示模式" }</span> }
                                } else { Html::default() } }
                            </div>
                        </div>
                        { if let Some(status) = &status {
                            if status.configured {
                                html! {
                                    <div class="config-meta">
                                        <span>{ format!("密钥：{}", status.api_key_masked.clone().unwrap_or_else(|| "未使用".to_string())) }</span>
                                        <span>{ format!("配置于 {}", status.configured_at.as_deref().map(format_time).unwrap_or_else(|| "—".to_string())) }</span>
                                    </div>
                                }
                            } else { Html::default() }
                        } else { Html::default() } }
                    </div>

                    <div class="config-card">
                        <form class="config-form" onsubmit={on_submit}>
                            <div class="preset-row">
                                <span class="preset-label">{ "快速填入" }</span>
                                { for PRESETS.iter().map(|(label, url, model_name, dims, wire_protocol)| {
                                    let base_url = base_url.clone();
                                    let model = model.clone();
                                    let dimensions = dimensions.clone();
                                    let protocol = protocol.clone();
                                    let url = url.to_string();
                                    let model_name = model_name.to_string();
                                    let dims = dims.to_string();
                                    let wire_protocol = wire_protocol.to_string();
                                    html! {
                                        <button type="button" class="preset-chip" onclick={Callback::from(move |_| {
                                            base_url.set(url.clone());
                                            model.set(model_name.clone());
                                            dimensions.set(dims.clone());
                                            protocol.set(wire_protocol.clone());
                                        })}>
                                            { *label }
                                        </button>
                                    }
                                }) }
                            </div>

                            <label class="field">
                                <span class="field-label">{ "Base URL" }</span>
                                <span class="field-hint">{ "填入 API 根地址，通常以 /v1 结尾" }</span>
                                <input
                                    class="input"
                                    type="url"
                                    placeholder="https://api.openai.com/v1"
                                    value={(*base_url).clone()}
                                    oninput={on_base_input}
                                    spellcheck="false"
                                    autocomplete="off"
                                    required=true
                                />
                            </label>

                            <label class="field">
                                <span class="field-label">{ "API Key" }</span>
                                <span class="field-hint">
                                    { if status.as_ref().and_then(|status| status.api_key_masked.as_ref()).is_some() {
                                        "已保存的密钥不会回显；重新保存时请重新粘贴"
                                    } else {
                                        "留空则不发送 Authorization（适合本地 Ollama）"
                                    } }
                                </span>
                                <span class="input-group">
                                    <input
                                        class="input"
                                        type={if *show_key { "text" } else { "password" }}
                                        placeholder="sk-..."
                                        value={(*api_key).clone()}
                                        oninput={on_key_input}
                                        spellcheck="false"
                                        autocomplete="new-password"
                                    />
                                    <button type="button" class="input-addon" onclick={toggle_key}
                                        aria-label={if *show_key { "隐藏密钥" } else { "显示密钥" }}>
                                        { if *show_key { icons::eye_off() } else { icons::eye() } }
                                    </button>
                                </span>
                            </label>

                            <label class="field">
                                <span class="field-label">{ "接口协议" }</span>
                                <select class="input select" onchange={on_protocol_change}>
                                    <option value="openai" selected={*protocol == "openai"}>
                                        { "OpenAI 兼容 · /embeddings" }
                                    </option>
                                    <option value="dashscope" selected={*protocol == "dashscope"}>
                                        { "DashScope 原生 · TextEmbedding" }
                                    </option>
                                </select>
                                <span class="field-hint">
                                    { if *protocol == "dashscope" {
                                        "自动追加 /services/embeddings/text-embedding/text-embedding；查询与文档分别使用 query / document"
                                    } else {
                                        "自动追加 /embeddings；适用于 OpenAI、通义兼容模式、SiliconFlow、Ollama、vLLM"
                                    } }
                                </span>
                            </label>

                            <div class="field-row">
                                <label class="field">
                                    <span class="field-label">{ "模型" }</span>
                                    <input
                                        class="input"
                                        type="text"
                                        placeholder="text-embedding-3-small"
                                        value={(*model).clone()}
                                        oninput={on_model_input}
                                        spellcheck="false"
                                        required=true
                                    />
                                </label>
                                <label class="field field-small">
                                    <span class="field-label">{ "维度（可选）" }</span>
                                    <input
                                        class="input"
                                        type="number"
                                        min="0"
                                        placeholder="自动"
                                        value={(*dimensions).clone()}
                                        oninput={on_dims_input}
                                    />
                                </label>
                            </div>

                            <div class="form-actions">
                                <button class="button button-primary" type="submit" disabled={*submitting}>
                                    { if *submitting { html! { <Spinner /> } } else { icons::pulse() } }
                                    { if *submitting { "正在连接…" } else { "保存并测试" } }
                                </button>
                                <button class="button button-ghost" type="button" onclick={on_mock}
                                    disabled={*submitting}>
                                    { "本地演示嵌入" }
                                </button>
                                { if configured {
                                    html! {
                                        <button class="button button-text-danger" type="button" onclick={on_clear}>
                                            { "清除配置" }
                                        </button>
                                    }
                                } else { Html::default() } }
                            </div>

                            { if let Some(message) = &*error {
                                html! { <p class="form-message is-error" role="alert">{ message.clone() }</p> }
                            } else { Html::default() } }
                            { if let Some(message) = &*note {
                                html! { <p class="form-message is-success">{ message.clone() }</p> }
                            } else { Html::default() } }

                            <p class="security-note">
                                { icons::shield() }
                                { "密钥仅保存在这个 Rust 进程的内存中；重启即清除。页面不会回显完整密钥。" }
                            </p>
                        </form>

                        { if let Some(response) = &*probe {
                            html! {
                                <div class="probe-card">
                                    <div class="probe-head">
                                        <div>
                                            <p class="probe-kicker">{ "实时探针" }</p>
                                            <h3>{ "第一条真实向量" }</h3>
                                        </div>
                                        <span class="chip chip-success">
                                            { format!("{} ms", response.latency_ms) }
                                        </span>
                                    </div>
                                    <VectorBars
                                        values={response.probe.sample.clone()}
                                        dimension={response.probe.dimension}
                                        norm={response.probe.norm}
                                    />
                                    <p class="probe-text">
                                        { format!("探针文本：{}", response.probe.text) }
                                    </p>
                                </div>
                            }
                        } else { Html::default() } }
                    </div>
                </div>
            </div>
        </section>
    }
}
