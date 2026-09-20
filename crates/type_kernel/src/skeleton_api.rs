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
//! reaches through seams stay crate-private. The class/member slice (#118)
//! additionally needs the wire writer: the skeleton constructs
//! `TypeInfoSnapshot`s for the classes it checks, and the snapshot carries
//! `bases` / `type_var_upper_bounds` as wire-format blobs, so the skeleton
//! must encode `Type` values it built itself. `encode_type` wraps the
//! crate-private writer (`write_type` + `WriteBuffer`, both pure Rust, no
//! PyO3 seam) behind one stable call; the writer types stay internal.

pub use crate::subtypes::{is_subtype, SubtypeContext};
pub use crate::typeinfo::{ModuleSnapshot, TypeInfoSnapshot, TypeResolver};
pub use crate::wire::Type;

/// Serialize `t` to the kernel wire format (`wire::write_type` over a
/// fresh buffer). The skeleton uses this to fill the blob fields of the
/// `TypeInfoSnapshot`s it constructs. Errors are unreadable-input cases
/// the wire writer refuses (never hit by skeleton-built types).
pub fn encode_type(t: &Type) -> Result<Vec<u8>, String> {
    let mut buf = crate::wire::WriteBuffer::new();
    crate::wire::write_type(&mut buf, t).map_err(|e| e.to_string())?;
    Ok(buf.into_bytes())
}
