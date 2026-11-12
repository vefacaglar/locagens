//! Background process manager now lives in `locagens-execpolicy`.
//!
//! Re-exported here so existing `services::processes::*` callers keep working
//! unchanged.

pub use locagens_execpolicy::processes::*;
