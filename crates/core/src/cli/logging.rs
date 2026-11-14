//! `tracing` output for `lcgn`: appended to `<locagens home>/logs/lcgn.log`,
//! never to the terminal (stderr would corrupt the TUI). `RUST_LOG` picks the
//! level (default `warn`).

use std::fs::OpenOptions;
use std::sync::Mutex;
use tracing_subscriber::EnvFilter;

/// Installs the file subscriber; silently does nothing when the log file
/// cannot be opened.
pub fn init() {
    let dir = crate::paths::locagens_home().join("logs");
    if std::fs::create_dir_all(&dir).is_err() {
        return;
    }
    let Ok(file) = OpenOptions::new().create(true).append(true).open(dir.join("lcgn.log")) else {
        return;
    };
    let filter = EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("warn"));
    let _ = tracing_subscriber::fmt()
        .with_env_filter(filter)
        .with_writer(Mutex::new(file))
        .with_ansi(false)
        .try_init();
}
