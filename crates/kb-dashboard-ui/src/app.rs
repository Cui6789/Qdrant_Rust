//! The mountable dashboard application and its single-page shell.

use gloo_timers::callback::Interval;
use kb_dashboard_types::OverviewResponse;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use crate::api::ApiClient;
use crate::components::browser::BrowserSection;
use crate::components::embedding::EmbeddingSection;
use crate::components::nav::Nav;
use crate::components::overview::OverviewSection;
use crate::components::pipeline::PipelineSection;
use crate::components::workbench::{Category, CategoryNav};

/// Properties of the mountable dashboard application.
#[derive(Properties, PartialEq, Default)]
pub struct DashboardAppProps {
    /// API root including `/api`, for example `http://127.0.0.1:8090/api`.
    ///
    /// When omitted, the app uses the current origin (or the `trunk serve`
    /// fallback to `http://<host>:8090/api`).
    #[prop_or_default]
    pub api_base: Option<String>,
}

/// Complete Qdrant_Rust workbench, ready to mount in any Yew application.
///
/// ```ignore
/// yew::Renderer::<kb_dashboard_ui::DashboardApp>::with_root(root).render();
/// ```
#[function_component(DashboardApp)]
pub fn dashboard_app(props: &DashboardAppProps) -> Html {
    let client = use_memo(props.api_base.clone(), |base| match base {
        Some(base) => ApiClient::new(base.clone()),
        None => ApiClient::default(),
    });

    html! {
        <ContextProvider<ApiClient> context={(*client).clone()}>
            <AppShell />
        </ContextProvider<ApiClient>>
    }
}

#[function_component(AppShell)]
fn app_shell() -> Html {
    let api = use_context::<ApiClient>().unwrap_or_default();
    let overview = use_state(|| None::<OverviewResponse>);
    let overview_error = use_state(|| None::<String>);
    let active_collection = use_state(|| None::<String>);
    let refresh_nonce = use_state(|| 0u32);
    let category = use_state(|| Category::Overview);

    let load_overview = {
        let api = api.clone();
        let overview = overview.clone();
        let overview_error = overview_error.clone();
        let active_collection = active_collection.clone();
        Callback::from(move |_| {
            let api = api.clone();
            let overview = overview.clone();
            let overview_error = overview_error.clone();
            let active_collection = active_collection.clone();
            spawn_local(async move {
                match api.overview().await {
                    Ok(data) => {
                        let current = (*active_collection).clone();
                        let next = match current.clone() {
                            Some(name)
                                if data
                                    .collections
                                    .iter()
                                    .any(|collection| collection.name == name) =>
                            {
                                Some(name)
                            }
                            _ => data
                                .collections
                                .first()
                                .map(|collection| collection.name.clone()),
                        };
                        if next != current {
                            active_collection.set(next);
                        }
                        overview.set(Some(data));
                        overview_error.set(None);
                    }
                    Err(failure) => overview_error.set(Some(failure.message)),
                }
            });
        })
    };

    {
        let load = load_overview.clone();
        // Restart the polling loop if the injected API endpoint changes.
        let api = api.clone();
        use_effect_with(api, move |_| {
            load.emit(());
            let tick = load.clone();
            let interval = Interval::new(4_000, move || tick.emit(()));
            move || drop(interval)
        });
    }

    let refresh_all = {
        let load = load_overview.clone();
        let nonce = refresh_nonce.clone();
        Callback::from(move |_| {
            load.emit(());
            nonce.set(*nonce + 1);
        })
    };
    let on_collection = {
        let active_collection = active_collection.clone();
        Callback::from(move |name: String| active_collection.set(Some(name)))
    };
    let on_overview_refresh = {
        let load = load_overview.clone();
        Callback::from(move |_| load.emit(()))
    };

    let set_category = {
        let category = category.clone();
        Callback::from(move |next: Category| category.set(next))
    };
    let open_ingest = {
        let set = set_category.clone();
        Callback::from(move |_| set.emit(Category::Ingest))
    };
    let open_search = {
        let set = set_category.clone();
        Callback::from(move |_| set.emit(Category::Search))
    };
    let open_model = {
        let set = set_category.clone();
        Callback::from(move |_| set.emit(Category::Model))
    };

    let nav = html! {
        <CategoryNav active={*category} on_select={set_category.clone()} />
    };

    let collections = overview
        .as_ref()
        .map(|data| data.collections.clone())
        .unwrap_or_default();
    let qdrant_ready = overview
        .as_ref()
        .map(|data| data.qdrant.ready)
        .unwrap_or(false);
    let embedding_configured = overview
        .as_ref()
        .map(|data| data.embedding.configured)
        .unwrap_or(false);

    html! {
        <div class="wb-shell">
            <Nav
                qdrant_ready={qdrant_ready}
                embedding_configured={embedding_configured}
                loaded={overview.is_some()}
            />
            <div class="wb-body">
                <div class={classes!("wb-view", (*category == Category::Overview).then_some("is-active"))}>
                    <OverviewSection
                        nav={nav.clone()}
                        overview={(*overview).clone()}
                        error={(*overview_error).clone()}
                        on_refresh={on_overview_refresh}
                        on_open_search={open_search.clone()}
                    />
                </div>
                <div class={classes!("wb-view", (*category == Category::Ingest).then_some("is-active"))}>
                    <PipelineSection
                        nav={nav.clone()}
                        collections={collections.clone()}
                        active_collection={(*active_collection).clone()}
                        embedding_configured={embedding_configured}
                        on_collection={on_collection.clone()}
                        on_done={refresh_all.clone()}
                        on_view_browser={open_search.clone()}
                        on_open_model={open_model.clone()}
                    />
                </div>
                <div class={classes!("wb-view", (*category == Category::Search).then_some("is-active"))}>
                    <BrowserSection
                        nav={nav.clone()}
                        collections={collections}
                        active_collection={(*active_collection).clone()}
                        on_collection={on_collection}
                        refresh_nonce={*refresh_nonce}
                        embedding_configured={embedding_configured}
                        on_changed={refresh_all.clone()}
                        on_open_ingest={open_ingest}
                        on_open_model={open_model}
                    />
                </div>
                <div class={classes!("wb-view", (*category == Category::Model).then_some("is-active"))}>
                    <EmbeddingSection nav={nav.clone()} on_saved={refresh_all.clone()} />
                </div>
            </div>
        </div>
    }
}
