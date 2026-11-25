use kb_dashboard_types::OverviewResponse;
use yew::prelude::*;

use crate::icons;
use crate::util::{format_duration, format_int};

#[derive(Properties, PartialEq)]
pub struct HeroProps {
    pub overview: Option<OverviewResponse>,
}

#[function_component(Hero)]
pub fn hero(props: &HeroProps) -> Html {
    let ready = props
        .overview
        .as_ref()
        .map(|overview| overview.qdrant.ready)
        .unwrap_or(false);
    let points = props
        .overview
        .as_ref()
        .map(|overview| format_int(overview.total_points))
        .unwrap_or_else(|| "—".to_string());
    let collections = props
        .overview
        .as_ref()
        .map(|overview| overview.collections.len().to_string())
        .unwrap_or_else(|| "—".to_string());
    let dimension = props
        .overview
        .as_ref()
        .map(|overview| overview.max_dimension.to_string())
        .unwrap_or_else(|| "—".to_string());
    let latency = props
        .overview
        .as_ref()
        .and_then(|overview| overview.qdrant.latency_ms)
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    let status_text = if props.overview.is_none() {
        "正在连接"
    } else if ready {
        "实时在线"
    } else {
        "等待 Qdrant"
    };
    html! {
        <section id="top" class="hero">
            <div class="hero-glow hero-glow-a"></div>
            <div class="hero-glow hero-glow-b"></div>
            <div class="container hero-inner">
                <p class="hero-eyebrow">
                    <span class={classes!("status-dot", ready.then_some("is-live"))}></span>
                    { "Qdrant · Rust · 实时可视" }
                </p>
                <h1 class="hero-title">
                    { "让向量，" }
                    <span class="gradient-text">{ "看得见。" }</span>
                </h1>
                <p class="hero-sub">
                    { "从一篇文档，到一条可以被检索的向量。" }
                    <br class="desktop-break" />
                    { "实时查看向量库、接入任意 OpenAI 兼容嵌入模型，并亲眼见证每一跳。" }
                </p>
                <div class="hero-actions">
                    <a class="button button-primary" href="#pipeline">
                        { "开始入库" }
                        { icons::arrow_right() }
                    </a>
                    <a class="button button-link" href="#browser">
                        { "浏览向量库" }
                        { icons::chevron_right() }
                    </a>
                </div>

                <div class="hero-stage">
                    <div class="stage-chrome">
                        <div class="stage-dots" aria-hidden="true">
                            <i></i><i></i><i></i>
                        </div>
                        <span class="stage-title">{ "kb-dashboard.local" }</span>
                        <span class={classes!("stage-live", ready.then_some("is-live"))}>
                            <i class="status-dot"></i>
                            { status_text }
                        </span>
                    </div>
                    <div class="stage-body">
                        <div class="stage-stat">
                            <p class="stage-kicker">{ "向量点总数" }</p>
                            <p class="stage-big">{ points }</p>
                            <p class="stage-caption">
                                { format!("{} 个集合 · 最大 {} 维", collections, dimension) }
                            </p>
                            <div class="stage-mini-grid">
                                <div class="stage-mini">
                                    <span class="stage-mini-label">{ "Qdrant 延迟" }</span>
                                    <span class="stage-mini-value">{ latency }</span>
                                </div>
                                <div class="stage-mini">
                                    <span class="stage-mini-label">{ "刷新方式" }</span>
                                    <span class="stage-mini-value">{ "轮询 · 4s" }</span>
                                </div>
                            </div>
                        </div>

                        <div class="stage-orb-wrap" aria-hidden="true">
                            <div class="stage-orb">
                                <span class="orb-core"></span>
                                <span class="orb-ring ring-a"></span>
                                <span class="orb-ring ring-b"></span>
                                <span class="orb-ring ring-c"></span>
                                <span class="orb-particle particle-a"></span>
                                <span class="orb-particle particle-b"></span>
                                <span class="orb-particle particle-c"></span>
                                <span class="orb-label label-input">{ "输入" }</span>
                                <span class="orb-label label-embed">{ "嵌入" }</span>
                                <span class="orb-label label-write">{ "写入" }</span>
                            </div>
                            <div class="orb-caption">
                                <span class="mono">{ "text → vector → point" }</span>
                            </div>
                        </div>

                        <div class="stage-flow">
                            <div class="flow-step">
                                <span class="flow-index">{ "01" }</span>
                                <div>
                                    <p class="flow-title">{ "输入与切块" }</p>
                                    <p class="flow-text">{ "标题感知 · 中文友好" }</p>
                                </div>
                            </div>
                            <div class="flow-step">
                                <span class="flow-index">{ "02" }</span>
                                <div>
                                    <p class="flow-title">{ "API 嵌入" }</p>
                                    <p class="flow-text">{ "OpenAI 兼容 · 自动探测维度" }</p>
                                </div>
                            </div>
                            <div class="flow-step">
                                <span class="flow-index">{ "03" }</span>
                                <div>
                                    <p class="flow-title">{ "写入 Qdrant" }</p>
                                    <p class="flow-text">{ "确定性 ID · 幂等对账" }</p>
                                </div>
                            </div>
                            <div class="flow-line" aria-hidden="true">
                                <span class="flow-pulse"></span>
                            </div>
                        </div>
                    </div>
                    <div class="stage-foot">
                        <span class="stage-foot-label">{ "Pipeline" }</span>
                        <span class="stage-chip">{ "文本" }</span>
                        <span class="stage-arrow">{ "→" }</span>
                        <span class="stage-chip">{ "切块" }</span>
                        <span class="stage-arrow">{ "→" }</span>
                        <span class="stage-chip">{ "嵌入模型" }</span>
                        <span class="stage-arrow">{ "→" }</span>
                        <span class="stage-chip stage-chip-accent">{ "Qdrant" }</span>
                        <span class="stage-foot-note">{ "全程可视化" }</span>
                    </div>
                </div>
            </div>
        </section>
    }
}
