//! Yew entry point for the Qdrant_Rust visual dashboard.

mod api;
mod components;
mod icons;
mod util;

use gloo_timers::callback::Interval;
use kb_dashboard_types::OverviewResponse;
use wasm_bindgen_futures::spawn_local;
use yew::prelude::*;

use components::browser::BrowserSection;
use components::embedding::EmbeddingSection;
use components::features::Features;
use components::footer::Footer;
use components::hero::Hero;
use components::nav::Nav;
use components::overview::OverviewSection;
use components::pipeline::PipelineSection;

#[function_component(App)]
fn app() -> Html {
    let overview = use_state(|| None::<OverviewResponse>);
    let overview_error = use_state(|| None::<String>);
    let active_collection = use_state(|| None::<String>);
    let refresh_nonce = use_state(|| 0u32);

    let load_overview = {
        let overview = overview.clone();
        let overview_error = overview_error.clone();
        let active_collection = active_collection.clone();
        Callback::from(move |_| {
            let overview = overview.clone();
            let overview_error = overview_error.clone();
            let active_collection = active_collection.clone();
            spawn_local(async move {
                match api::overview().await {
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
        use_effect_with((), move |_| {
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
        <>
            <Nav
                qdrant_ready={qdrant_ready}
                embedding_configured={embedding_configured}
                loaded={overview.is_some()}
            />
            <main>
                <Hero overview={(*overview).clone()} />
                <Features />
                <OverviewSection
                    overview={(*overview).clone()}
                    error={(*overview_error).clone()}
                    on_refresh={on_overview_refresh}
                />
                <PipelineSection
                    collections={collections.clone()}
                    active_collection={(*active_collection).clone()}
                    embedding_configured={embedding_configured}
                    on_collection={on_collection.clone()}
                    on_done={refresh_all.clone()}
                />
                <EmbeddingSection on_saved={refresh_all.clone()} />
                <BrowserSection
                    collections={collections}
                    active_collection={(*active_collection).clone()}
                    on_collection={on_collection}
                    refresh_nonce={*refresh_nonce}
                    embedding_configured={embedding_configured}
                    on_changed={refresh_all}
                />
            </main>
            <Footer />
        </>
    }
}

fn main() {
    let root = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("app"))
        .expect("missing #app mount point");
    yew::Renderer::<App>::with_root(root).render();
}
