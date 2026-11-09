//! Library crate behind the `pipe-explorer` binary.
//!
//! The application lives here rather than in `main.rs` so that the
//! benchmarks in `benches/` can exercise its internals directly.

pub mod ansi;
pub mod app;
pub mod editor;
pub mod executor;
pub mod history;
pub mod pipeline;
pub mod search;
pub mod ui;
