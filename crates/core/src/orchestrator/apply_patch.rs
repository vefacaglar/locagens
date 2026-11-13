//! Codex `apply_patch` engine — thin re-export shim.
//!
//! The implementation lives in `crates/apply-patch`
//! (`locagens-apply-patch`) so it has no dependency on `locagens-core`. Everything
//! public in that crate is re-exported here, so `crate::orchestrator::
//! apply_patch::*` keeps resolving exactly as before.

pub use locagens_apply_patch::*;
