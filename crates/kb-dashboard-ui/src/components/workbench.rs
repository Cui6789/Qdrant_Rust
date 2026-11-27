//! Shared pieces for the single-page workbench shell.

use yew::prelude::*;

use crate::icons;

/// The workbench categories shown in the left pane.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Category {
    Overview,
    Ingest,
    Search,
}

impl Category {
    /// Categories in sidebar order.
    pub fn all() -> [Category; 3] {
        [Category::Overview, Category::Ingest, Category::Search]
    }

    /// Short label shown on the category button.
    pub fn label(self) -> &'static str {
        match self {
            Self::Overview => "概览",
            Self::Ingest => "入库",
            Self::Search => "检索",
        }
    }

    /// One-line description shown under the label.
    pub fn hint(self) -> &'static str {
        match self {
            Self::Overview => "连接与集合状态",
            Self::Ingest => "写入一篇文档",
            Self::Search => "检索与浏览数据库",
        }
    }

    fn icon(self) -> Html {
        match self {
            Self::Overview => icons::activity(),
            Self::Ingest => icons::upload(),
            Self::Search => icons::search(),
        }
    }
}

#[derive(Properties, PartialEq)]
pub struct CategoryNavProps {
    pub active: Category,
    pub on_select: Callback<Category>,
}

/// Left-pane category switcher. Clicking a category swaps the right-hand view.
#[function_component(CategoryNav)]
pub fn category_nav(props: &CategoryNavProps) -> Html {
    html! {
        <nav class="wb-cat-nav" aria-label="工作台类别">
            <p class="wb-kicker">{ "Workspace" }</p>
            <div class="wb-cat-grid">
                { for Category::all().into_iter().map(|category| {
                    let selected = category == props.active;
                    let on_select = props.on_select.clone();
                    html! {
                        <button type="button"
                            class={classes!("wb-cat-button", selected.then_some("is-active"))}
                            aria-pressed={selected.to_string()}
                            onclick={Callback::from(move |_| on_select.emit(category))}>
                            <span class="wb-cat-icon">{ category.icon() }</span>
                            <span class="wb-cat-copy">
                                <span class="wb-cat-label">{ category.label() }</span>
                                <span class="wb-cat-hint">{ category.hint() }</span>
                            </span>
                        </button>
                    }
                }) }
            </div>
        </nav>
    }
}

#[derive(Properties, PartialEq)]
pub struct MainHeadProps {
    pub kicker: String,
    pub title: Html,
    #[prop_or_default]
    pub subtitle: Option<String>,
    #[prop_or_default]
    pub actions: Html,
}

/// Heading strip at the top of the right pane.
#[function_component(MainHead)]
pub fn main_head(props: &MainHeadProps) -> Html {
    html! {
        <div class="wb-main-head">
            <div class="wb-main-copy">
                <p class="wb-kicker">{ props.kicker.clone() }</p>
                <h1>{ props.title.clone() }</h1>
                { if let Some(subtitle) = &props.subtitle {
                    html! { <p class="wb-main-sub">{ subtitle.clone() }</p> }
                } else { Html::default() } }
            </div>
            { props.actions.clone() }
        </div>
    }
}

#[derive(Properties, PartialEq)]
pub struct PanePanelProps {
    pub kicker: String,
    pub title: String,
    #[prop_or_default]
    pub actions: Html,
    #[prop_or_default]
    pub children: Children,
}

/// Card used by the left-pane control groups.
#[function_component(PanePanel)]
pub fn pane_panel(props: &PanePanelProps) -> Html {
    html! {
        <section class="wb-panel">
            <div class="wb-panel-head">
                <div>
                    <p class="wb-kicker">{ props.kicker.clone() }</p>
                    <h2>{ props.title.clone() }</h2>
                </div>
                { props.actions.clone() }
            </div>
            { for props.children.iter() }
        </section>
    }
}
