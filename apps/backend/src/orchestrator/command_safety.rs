//! Command safety classification now lives in `locagens-execpolicy`.
//!
//! Re-exported here so existing `orchestrator::command_safety::*` callers
//! keep working unchanged.

pub use locagens_execpolicy::command_safety::*;
