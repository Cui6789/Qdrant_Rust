//! Reusable Yew components for the workbench.
//!
//! The four feature sections are self-contained: each owns its form/query
//! state, reads the API client from Yew context, and reports user actions
//! through callbacks. The remaining modules provide the shell layout, icons and
//! presentational primitives.

pub mod browser;
pub mod embedding;
pub mod nav;
pub mod overview;
pub mod pipeline;
pub mod ui;
pub mod workbench;

pub use browser::BrowserSection;
pub use embedding::EmbeddingSection;
pub use nav::Nav;
pub use overview::OverviewSection;
pub use pipeline::PipelineSection;
pub use ui::{CollectionBars, EmptyState, SectionHeading, Spinner, StatTile, VectorBars};
pub use workbench::{Category, CategoryNav, MainHead, PanePanel};
