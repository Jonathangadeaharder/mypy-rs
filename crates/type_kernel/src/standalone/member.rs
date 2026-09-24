//! Standalone-path public API: member.
//!
//! Attribute and member access: instance vs class, properties,
//! descriptors, __getattr__.
//!
//! Contract (wave 1, docs/plans/2026-09-24-standalone-full-port-
//! wave1.md): this module re-exposes already-ported kernel logic for a
//! caller that has no Python interpreter. Every public item takes and
//! returns pure-Rust kernel types only: no pyo3 type may appear in a
//! public signature, and nothing here registers a seam or touches the
//! hybrid check path. Lift the existing `*_inner` and private helpers out
//! of checkmember.rs, checker_helpers.rs, attrs.rs, classmethod_static.rs
//! rather than reimplementing them; where a helper needs a Python-side
//! callback today, take the callback as a Rust trait object or an
//! explicit record and say so in the doc comment.
//!
//! Owned exclusively by the `member` lane for this wave. Add
//! `#[cfg(test)]` unit tests here: they keep the lifted API honest and
//! are the only thing that exercises it before the driver integration
//! wave.
