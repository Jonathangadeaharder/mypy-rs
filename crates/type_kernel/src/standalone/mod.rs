//! Public Rust API of the standalone (Python-free) check path.
//!
//! One submodule per ported area. The hybrid reaches the same logic
//! through its `#[pyfunction]` seams; the standalone driver reaches it
//! through here, with no wire encode/decode round trip and no Python
//! objects. Adding an area means adding a submodule file and one `pub
//! mod` line below, kept in alphabetical order (rustfmt reorders them).
//!
//! See docs/plans/2026-09-24-standalone-full-port-wave1.md.

pub mod call;
pub mod deps;
pub mod diag;
pub mod expr;
pub mod infer;
pub mod member;
pub mod narrow;
pub mod pattern;
pub mod records;
pub mod semanal;
pub mod stmt;
pub mod types;
