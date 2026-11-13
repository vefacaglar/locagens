//! `locagens-execpolicy` — command safety classification and the background
//! process manager, extracted from the Locagens backend so the desktop app
//! and CLI can share them.

pub mod command_safety;
pub mod processes;
pub mod rules;
pub mod sandbox;

pub use command_safety::*;
pub use processes::*;
