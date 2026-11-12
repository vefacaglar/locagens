//! `lcgn` — the Locagens coding agent CLI (pi-like, on the shared Rust core).

fn main() {
    std::process::exit(locagens_core::cli::main_entry());
}
