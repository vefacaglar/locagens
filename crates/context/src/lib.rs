//! `locagens-context` — run context helpers shared by the Locagens backend.
//!
//! Contains the Codex-derived context pieces that are independent of the
//! orchestrator runtime:
//!
//! - [`agents_md`]: hierarchical `AGENTS.md` discovery (Codex's
//!   `project_doc_max_bytes` semantics).
//! - [`env_context`]: the `<environment_context>` XML block seeded as the
//!   first user message of a run.
//! - [`turn_diff_tracker`]: per-turn workspace diff tracking.

pub mod agents_md;
pub mod env_context;
pub mod turn_diff_tracker;
