//! Standalone driver glue: lower.
//!
//! Complete ruff-ast lowering: every expression and statement form the
//! port supports.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module is the skeleton-side caller of
//! `type_kernel::standalone::records`. It owns no checking logic of its
//! own: it adapts skeleton records (`crate::model`, `crate::fixtures`) to
//! the kernel API and adapts the kernel's answers back to diagnostics.
//! Integration into `crate::check`'s `Driver` happens in the wave-2
//! integration lane, so nothing here may edit `check.rs`, `main.rs` or
//! `subset.rs`.
//!
//! Owned exclusively by the `lower` lane for this wave. Add
//! `#[cfg(test)]` unit tests here.
