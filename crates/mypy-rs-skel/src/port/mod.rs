//! Skeleton-side driver glue for the standalone port.
//!
//! One submodule per ported area, mirroring `type_kernel::standalone`.
//! These modules adapt skeleton records to the kernel's standalone API
//! and are wired into `crate::check`'s `Driver` by the integration wave.

pub mod member;
pub mod call;
pub mod infer;
pub mod types;
pub mod narrow;
pub mod stmt;
pub mod semanal;
pub mod diag;
pub mod deps;
pub mod pattern;
pub mod expr;
pub mod lower;
pub mod stubs;
