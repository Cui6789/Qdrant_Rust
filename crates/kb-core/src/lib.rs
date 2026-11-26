//! RAG data layer for Qdrant_Rust: parsing, chunking, embeddings, idempotent
//! ingestion, Qdrant storage and retrieval.
//!
//! See `README.md` in this crate for direct database-operation examples.

pub mod config;
pub mod embedding;
pub mod error;
pub mod ingest;
pub mod models;
pub mod service;
pub mod store;

pub use error::{KbError, Result};
