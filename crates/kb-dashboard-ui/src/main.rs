//! Yew entry point for the Qdrant_Rust visual dashboard.
//!
//! All reusable code lives in the library crate (`src/lib.rs`); this file only
//! mounts [`DashboardApp`] into the page.

use kb_dashboard_ui::DashboardApp;

fn main() {
    let root = web_sys::window()
        .and_then(|window| window.document())
        .and_then(|document| document.get_element_by_id("app"))
        .expect("missing #app mount point");
    yew::Renderer::<DashboardApp>::with_root(root).render();
}
