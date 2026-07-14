//! Host-neutral HTTP and CLI boundary for Aeon Memory.
//!
//! Business behavior is deliberately supplied through [`AeonMemoryService`]. This
//! crate never substitutes transport-layer mock behavior for the memory core.

pub mod adapter;
pub mod api;
pub mod cli;
pub mod config;
pub mod runtime;
pub mod service;

pub use api::{AppConfig, ROUTES, app};
pub use service::*;
