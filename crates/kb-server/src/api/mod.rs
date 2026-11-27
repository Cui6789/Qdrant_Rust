//! HTTP API: routing, handlers and middleware.

mod admin;
pub mod handlers;
pub mod routes;
mod upload;

pub use routes::{build_managed_router, build_router, with_common_layers};
