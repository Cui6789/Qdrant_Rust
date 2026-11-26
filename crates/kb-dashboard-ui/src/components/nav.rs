//! Slim top bar with the brand and live Qdrant/embedding status.

use yew::prelude::*;

use crate::icons;

#[derive(Properties, PartialEq)]
pub struct NavProps {
    pub qdrant_ready: bool,
    pub embedding_configured: bool,
    pub loaded: bool,
}

/// Slim top bar: brand, Qdrant state and embedding state.
#[function_component(Nav)]
pub fn nav(props: &NavProps) -> Html {
    let qdrant_class = if !props.loaded {
        "nav-pill is-muted"
    } else if props.qdrant_ready {
        "nav-pill is-online"
    } else {
        "nav-pill is-offline"
    };
    let qdrant_text = if !props.loaded {
        "连接中"
    } else if props.qdrant_ready {
        "Qdrant 在线"
    } else {
        "Qdrant 离线"
    };
    let embedding_class = if props.embedding_configured {
        "nav-pill is-ready"
    } else {
        "nav-pill is-muted"
    };
    let embedding_text = if props.embedding_configured {
        "嵌入已就绪"
    } else {
        "待接入模型"
    };
    html! {
        <header class="nav">
            <div class="nav-inner">
                <div class="brand" aria-label="Qdrant_Rust Studio 工作台">
                    { icons::logo() }
                    <span class="brand-text">
                        <span class="brand-name">{ "Qdrant_Rust" }</span>
                        <span class="brand-sub">{ "Studio · 工作台" }</span>
                    </span>
                </div>
                <div class="nav-status">
                    <span class={qdrant_class} title="Qdrant gRPC 连接状态">
                        <i class="status-dot"></i>
                        { qdrant_text }
                    </span>
                    <span class={embedding_class} title="当前嵌入模型配置状态">
                        { embedding_text }
                    </span>
                </div>
            </div>
        </header>
    }
}
