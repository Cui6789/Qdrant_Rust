//! HTTP API: routing, handlers and middleware.

pub mod handlers;
pub mod routes;

pub use routes::{build_router, with_common_layers};
