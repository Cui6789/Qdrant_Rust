//! Reusable Yew/WASM modules for the Qdrant_Rust visual dashboard.
//!
//! # Entry points
//!
//! - [`DashboardApp`] mounts the complete workbench.
//! - [`ApiClient`] is the typed HTTP client; it is provided through Yew context
//!   by [`DashboardApp`] and can be injected directly when composing sections.
//! - [`components`] contains the shell, the four feature sections, and the
//!   presentational primitives they share.
//!
//! See `docs/dashboard.md` for the workbench's runtime and API documentation.

pub mod api;
mod app;
pub mod components;
pub mod icons;
pub mod util;

pub use api::{ApiClient, ApiError};
pub use app::{DashboardApp, DashboardAppProps};
