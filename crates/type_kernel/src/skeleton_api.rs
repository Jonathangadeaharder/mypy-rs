//! Public Rust surface for the standalone skeleton crate
//! (`crates/mypy-rs-skel`, issue #91, ADR-0008 Lane 2).
//!
//! Visibility only: every item keeps its definition and its behavior in
//! its own module. Nothing here registers a PyO3 seam, touches the wire
//! format, or changes any hybrid code path; the production check and the
//! registered-seam count are unchanged. The skeleton consumes the kernel
//! as an rlib and drives these entries with data loaded from committed
//! fixtures, per the skeleton brief (docs/plans/
//! 2026-09-19-standalone-skeleton-brief.md, section 2).
//!
//! The re-exports live here (not `pub mod` on the defining modules) so the
//! exposed surface stays this list; kernel internals that only the hybrid
//! reaches through seams stay crate-private.

pub use crate::subtypes::{is_subtype, SubtypeContext};
pub use crate::typeinfo::{ModuleSnapshot, TypeInfoSnapshot, TypeResolver};
pub use crate::wire::Type;
