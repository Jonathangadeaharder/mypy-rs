//! Skeleton-side driver glue for the standalone port.
//!
//! One submodule per ported area, mirroring `type_kernel::standalone`.
//! These modules adapt skeleton records to the kernel's standalone API
//! and are wired into `crate::check`'s `Driver` by the integration wave.
//! Kept in alphabetical order (rustfmt reorders them).

pub mod call;
pub mod deps;
pub mod diag;
pub mod expr;
pub mod infer;
pub mod lower;
pub mod member;
pub mod narrow;
pub mod pattern;
pub mod semanal;
pub mod stmt;
pub mod stubs;
pub mod types;
