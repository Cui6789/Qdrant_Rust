use kb_dashboard_types::OverviewResponse;
use yew::prelude::*;

use crate::components::ui::{CollectionBars, SectionHeading, Spinner, StatTile};
use crate::icons;
use crate::util::{embedding_label, format_duration, format_int, format_time, host_label};

#[derive(Properties, PartialEq)]
pub struct OverviewProps {
    pub overview: Option<OverviewResponse>,
    pub error: Option<String>,
    pub on_refresh: Callback<()>,
}

#[function_component(OverviewSection)]
pub fn overview_section(props: &OverviewProps) -> Html {
    let on_refresh = {
        let callback = props.on_refresh.clone();
        Callback::from(move |_| callback.emit(()))
    };
    let Some(overview) = props.overview.as_ref() else {
        return html! {
            <section id="overview" class="section overview-section">
                <div class="container">
                    <SectionHeading
                        eyebrow={"实时概览".to_string()}
                        title={html! { <>{"向量库的"}<span class="serif-accent">{"此刻"}</span>{"。"}</> }}
                        subtitle={html! { "每隔几秒读取一次 Qdrant，集合、向量点与连接状态都保持最新。" }}
                    />
                    { if let Some(error) = &props.error {
                        html! {
                            <div class="notice is-error">
                                <span class="notice-icon">{ icons::activity() }</span>
                                <div>
                                    <strong>{ "暂时读不到向量库" }</strong>
                                    <p>{ error.clone() }</p>
                                </div>
                                <button class="button button-small" onclick={on_refresh.clone()}>
                                    { icons::refresh() }{ "重试" }
                                </button>
                            </div>
                        }
                    } else {
                        html! {
                            <div class="loading-panel">
                                <Spinner />
                                <span>{ "正在读取 Qdrant…" }</span>
                            </div>
                        }
                    } }
                </div>
            </section>
        };
    };

    let embedding_dimension = overview
        .embedding
        .dimension
        .map(|dimension| format!("{dimension} 维"))
        .unwrap_or_else(|| "—".to_string());
    let qdrant_state = if overview.qdrant.ready {
        "在线"
    } else {
        "离线"
    };
    let qdrant_latency = overview
        .qdrant
        .latency_ms
        .map(format_duration)
        .unwrap_or_else(|| "—".to_string());
    let uptime_minutes = overview.server.uptime_secs / 60;
    let uptime = if uptime_minutes >= 60 {
        format!("{} 小时 {} 分", uptime_minutes / 60, uptime_minutes % 60)
    } else {
        format!("{uptime_minutes} 分钟")
    };
    let embedding_endpoint = overview
        .embedding
        .base_url
        .as_deref()
        .map(host_label)
        .unwrap_or_else(|| "尚未接入".to_string());
    let label = embedding_label(&overview.embedding);

    html! {
        <section id="overview" class="section overview-section">
            <div class="container">
                <SectionHeading
                    eyebrow={"实时概览".to_string()}
                    title={html! { <>{"向量库的"}<span class="serif-accent">{"此刻"}</span>{"。"}</> }}
                    subtitle={html! { "集合、向量点、维度与嵌入配置，全部直接读取 Qdrant 与当前进程。" }}
                />
                <div class="overview-toolbar reveal">
                    <span class={classes!("live-pill", overview.qdrant.ready.then_some("is-live"))}>
                        <i class="status-dot"></i>
                        { if overview.qdrant.ready { "实时" } else { "已降级" } }
                    </span>
                    <span class="overview-time">
                        { format!("最近读取 {}", format_time(&overview.scanned_at)) }
                    </span>
                    <button class="icon-button" title="立即刷新" onclick={on_refresh}>
                        { icons::refresh() }
                    </button>
                </div>

                { if !overview.qdrant.ready {
                    html! {
                        <div class="notice is-warning">
                            <span class="notice-icon">{ icons::database() }</span>
                            <div>
                                <strong>{ "Qdrant 当前离线" }</strong>
                                <p>
                                    { overview.qdrant.error.clone().unwrap_or_else(|| "请确认 gRPC 端口 6334 已启动。".to_string()) }
                                </p>
                            </div>
                            <span class="mono notice-mono">{ host_label(&overview.qdrant.url) }</span>
                        </div>
                    }
                } else { Html::default() } }

                <div class="stat-grid">
                    <StatTile
                        label={"集合".to_string()}
                        value={overview.collections.len().to_string()}
                        hint={Some("Collections".to_string())}
                        icon={icons::layers()}
                    />
                    <StatTile
                        label={"向量点".to_string()}
                        value={format_int(overview.total_points)}
                        hint={Some("Points".to_string())}
                        icon={icons::database()}
                        accent={true}
                    />
                    <StatTile
                        label={"最大向量维度".to_string()}
                        value={overview.max_dimension.to_string()}
                        hint={Some("Dimension".to_string())}
                        icon={icons::cpu()}
                    />
                    <StatTile
                        label={"当前嵌入模型".to_string()}
                        value={label}
                        hint={Some(embedding_dimension)}
                        icon={icons::spark()}
                    />
                </div>

                <div class="overview-panels">
                    <div class="panel panel-chart reveal">
                        <CollectionBars collections={overview.collections.clone()} />
                        <p class="panel-note">
                            { "条形长度按各集合向量点数量等比缩放，数值为实时读取结果。" }
                        </p>
                    </div>
                    <div class="panel panel-status reveal">
                        <div class="panel-head">
                            <h3>{ "系统状态" }</h3>
                            <span class="chip chip-muted">{ format!("v{}", overview.server.version) }</span>
                        </div>
                        <div class="status-row">
                            <span class="status-row-icon">{ icons::database() }</span>
                            <div class="status-row-body">
                                <span class="status-row-label">{ "Qdrant gRPC" }</span>
                                <span class="status-row-value mono">{ host_label(&overview.qdrant.url) }</span>
                            </div>
                            <span class={classes!("status-badge", overview.qdrant.ready.then_some("is-online"))}>
                                { qdrant_state }
                            </span>
                        </div>
                        <div class="status-row">
                            <span class="status-row-icon">{ icons::activity() }</span>
                            <div class="status-row-body">
                                <span class="status-row-label">{ "读取延迟" }</span>
                                <span class="status-row-value">{ qdrant_latency }</span>
                            </div>
                        </div>
                        <div class="status-row">
                            <span class="status-row-icon">{ icons::key() }</span>
                            <div class="status-row-body">
                                <span class="status-row-label">{ "嵌入服务" }</span>
                                <span class="status-row-value mono">
                                    { if overview.embedding.configured { embedding_endpoint } else { "尚未接入".to_string() } }
                                </span>
                            </div>
                            <span class={classes!("status-badge", overview.embedding.configured.then_some("is-online"))}>
                                { if overview.embedding.configured { "就绪" } else { "待配置" } }
                            </span>
                        </div>
                        <div class="status-row">
                            <span class="status-row-icon">{ icons::clock() }</span>
                            <div class="status-row-body">
                                <span class="status-row-label">{ "仪表盘运行" }</span>
                                <span class="status-row-value">{ uptime }</span>
                            </div>
                        </div>
                        { if overview.embedding.api_key_masked.is_some() {
                            html! {
                                <p class="key-hint">
                                    { icons::shield() }
                                    { format!("密钥仅驻留本机内存：{}", overview.embedding.api_key_masked.clone().unwrap_or_default()) }
                                </p>
                            }
                        } else { Html::default() } }
                    </div>
                </div>
            </div>
        </section>
    }
}
