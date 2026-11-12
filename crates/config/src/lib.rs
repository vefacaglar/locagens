//! Locagens configuration crate.
//!
//! Hosts the platform path resolution ([`paths`]) and the pure Rust
//! provider / auth / model engine ([`providers`]) shared by the backend,
//! the desktop app and future consumers.

pub mod paths;
pub mod providers;

/// Backwards-compatible alias: the provider engine used to live at
/// `backend::pi_providers`.
pub use providers as pi_providers;
