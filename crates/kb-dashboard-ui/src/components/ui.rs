//! Small shared presentation components, including the two charts.

use kb_dashboard_types::CollectionSummary;
use yew::prelude::*;

use crate::util::format_int;

#[derive(Properties, PartialEq)]
pub struct SectionHeadingProps {
    pub eyebrow: String,
    pub title: Html,
    pub subtitle: Html,
    #[prop_or(false)]
    pub centered: bool,
}

#[function_component(SectionHeading)]
pub fn section_heading(props: &SectionHeadingProps) -> Html {
    let class = if props.centered {
        "section-heading is-centered"
    } else {
        "section-heading"
    };
    html! {
        <div class={class}>
            <p class="eyebrow">{ props.eyebrow.clone() }</p>
            <h2>{ props.title.clone() }</h2>
            <p class="section-sub">{ props.subtitle.clone() }</p>
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct StatTileProps {
    pub label: String,
    pub value: String,
    #[prop_or_default]
    pub hint: Option<String>,
    pub icon: Html,
    #[prop_or_default]
    pub accent: bool,
}

#[function_component(StatTile)]
pub fn stat_tile(props: &StatTileProps) -> Html {
    let class = classes!("stat-tile", props.accent.then_some("is-accent"));
    html! {
        <div class={class}>
            <div class="stat-tile-top">
                <span class="stat-icon">{ props.icon.clone() }</span>
                { props.hint.as_ref().map(|hint| html! { <span class="stat-hint">{ hint.clone() }</span> }) }
            </div>
            <div class="stat-value" title={props.value.clone()}>{ props.value.clone() }</div>
            <div class="stat-label">{ props.label.clone() }</div>
        </div>
    }
}

#[function_component(Spinner)]
pub fn spinner() -> Html {
    html! { <span class="spinner" aria-label="载入中"></span> }
}

#[derive(Properties, PartialEq)]
pub struct VectorBarsProps {
    pub values: Vec<f32>,
    #[prop_or_default]
    pub dimension: usize,
    #[prop_or_default]
    pub norm: f32,
}

/// A signed magnitude chart for the leading dimensions of one embedding.
///
/// Positive values grow upward in the accent blue, negative values grow
/// downward in a muted indigo, and the zero line is neutral. Each bar exposes
/// its dimension and value through a native hover title.
#[function_component(VectorBars)]
pub fn vector_bars(props: &VectorBarsProps) -> Html {
    if props.values.is_empty() {
        return html! {
            <div class="vector-empty">
                <span>{ "运行一次入库后，这里会显示首块向量的真实数值。" }</span>
            </div>
        };
    }
    let max_abs = props
        .values
        .iter()
        .fold(0.0_f32, |current, value| current.max(value.abs()))
        .max(0.001);
    let bars = props
        .values
        .iter()
        .enumerate()
        .map(|(index, value)| {
            let is_positive = *value >= 0.0;
            let height = (value.abs() / max_abs * 100.0).max(1.5);
            let style = format!(
                "height:{height:.2}%;{}",
                if is_positive {
                    "bottom:50%;border-radius:4px 4px 1px 1px;"
                } else {
                    "top:50%;border-radius:1px 1px 4px 4px;"
                }
            );
            let class = if is_positive {
                "vbar-fill is-positive"
            } else {
                "vbar-fill is-negative"
            };
            let title = format!("维度 {index}：{value:.4}");
            html! {
                <div class="vbar-track" title={title}>
                    <span class={class} style={style}></span>
                </div>
            }
        })
        .collect::<Html>();
    let dimension = if props.dimension == 0 {
        props.values.len()
    } else {
        props.dimension
    };
    html! {
        <div class="vector-chart">
            <div class="vector-chart-head">
                <span class="chart-caption">
                    { format!("{} 维 · 展示前 {} 维", format_int(dimension as u64), props.values.len()) }
                </span>
                <span class="chart-norm">{ format!("L2 范数 {:.3}", props.norm) }</span>
            </div>
            <div class="vector-bars" role="img" aria-label="首块向量前若干维的数值">
                <span class="vbar-axis vbar-axis-top">{ "1" }</span>
                <div class="vbar-plot">{ bars }</div>
                <span class="vbar-zero"></span>
                <span class="vbar-axis vbar-axis-bottom">{ "-1" }</span>
            </div>
            <div class="vector-legend">
                <span><i class="swatch swatch-positive"></i>{ "正值" }</span>
                <span><i class="swatch swatch-negative"></i>{ "负值" }</span>
                <span class="legend-note">{ "悬停查看维度与数值" }</span>
            </div>
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct CollectionBarsProps {
    pub collections: Vec<CollectionSummary>,
}

/// Horizontal magnitude bars for points per collection (single hue, direct labels).
#[function_component(CollectionBars)]
pub fn collection_bars(props: &CollectionBarsProps) -> Html {
    if props.collections.is_empty() {
        return html! {
            <div class="collection-chart-empty">
                <span>{ "还没有集合。入库第一篇文档时，集合会自动出现。" }</span>
            </div>
        };
    }
    let max = props
        .collections
        .iter()
        .map(|collection| collection.points_count)
        .max()
        .unwrap_or(0)
        .max(1);
    let rows = props
        .collections
        .iter()
        .take(8)
        .map(|collection| {
            let width = (collection.points_count as f64 / max as f64 * 100.0).max(2.0);
            let title = format!(
                "{} · {} 个向量点 · {} 维",
                collection.name,
                format_int(collection.points_count),
                format_int(collection.vector_size)
            );
            html! {
                <div class="collection-row" title={title}>
                    <span class="collection-row-name">{ collection.name.clone() }</span>
                    <div class="collection-row-track">
                        <span class="collection-row-fill" style={format!("width:{width:.2}%")}></span>
                    </div>
                    <span class="collection-row-value">{ format_int(collection.points_count) }</span>
                </div>
            }
        })
        .collect::<Html>();
    html! {
        <div class="collection-chart">
            <div class="chart-head">
                <span class="chart-caption">{ "各集合向量点" }</span>
                <span class="chart-note">{ "实时读取 Qdrant" }</span>
            </div>
            { rows }
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct EmptyStateProps {
    pub icon: Html,
    pub title: String,
    pub text: String,
}

#[function_component(EmptyState)]
pub fn empty_state(props: &EmptyStateProps) -> Html {
    html! {
        <div class="empty-state">
            <span class="empty-icon">{ props.icon.clone() }</span>
            <h3>{ props.title.clone() }</h3>
            <p>{ props.text.clone() }</p>
        </div>
    }
}
