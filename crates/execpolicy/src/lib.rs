//! `locagens-execpolicy` — command safety classification, the background
//! process manager and command sandboxing (Seatbelt / Bubblewrap / Windows
//! restricted token + per-domain network proxy), extracted from the
//! Locagens backend so the desktop app and CLI can share them.

pub mod command_safety;
pub mod env_policy;
pub mod network_proxy;
pub mod processes;
pub mod protected_paths;
pub mod rules;
pub mod sandbox;
pub mod windows_sandbox;

pub use command_safety::*;
pub use processes::*;
