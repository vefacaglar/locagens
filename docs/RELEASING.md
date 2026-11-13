# Locagens — Release Guide

How to release the Locagens desktop app and CLI. The product is **100% pure
Rust**, released as two binaries:

| Crate | Path | Binary |
| --- | --- | --- |
| `locagens-cli` | `apps/cli` | `lcgn` (the CLI; its code lives in the `locagens-core` library) |
| `locagens-desktop` | `apps/desktop-rs` | `locagens` (native Dioxus 0.7 desktop app) |

`locagens-core` (`crates/core`) and the other `crates/*` are internal libraries
and are not versioned per release. Versions are **not** workspace-inherited:
`apps/cli/Cargo.toml` and `apps/desktop-rs/Cargo.toml` each carry their own
`version`. There is no Node.js, Electron, web app or HTTP server to release.

---

## Method 1: Recommended (tag-triggered release)

1. Bump the `version` fields in `apps/cli/Cargo.toml` and
   `apps/desktop-rs/Cargo.toml` (e.g. `0.2.0` → `0.3.0`), then refresh the
   lock file with `cargo check --workspace`.
2. Commit:
   ```bash
   git add apps/cli/Cargo.toml apps/desktop-rs/Cargo.toml Cargo.lock
   git commit -m "bump: v0.3.0"
   ```
3. Tag and push:
   ```bash
   git tag v0.3.0
   git push origin v0.3.0
   ```
4. GitHub Actions triggers **"Release (macOS & Windows binaries)"**
   (`.github/workflows/release-mac.yml`), which:
   - Rewrites the first `version` line of `apps/cli/Cargo.toml` and
     `apps/desktop-rs/Cargo.toml` from the tag, so the binaries carry the tag
     version even if step 1 was skipped.
   - Builds `locagens` (desktop) and `lcgn` (CLI) release binaries for
     `aarch64-apple-darwin`, `x86_64-apple-darwin` and
     `x86_64-pc-windows-msvc`.
   - Uploads them as workflow artifacts.
   - Creates a GitHub Release with the binaries and auto-generated notes.

When done, download the binaries from the GitHub Releases page.

---

## Method 2: Manual (build only, no release)

To build without creating a release:

1. GitHub → Actions → **"Release (macOS & Windows binaries)"** workflow.
2. Click **Run workflow**.
3. Leave `tag_name` empty (build only, no release).
4. After the build, download the binaries from the workflow **Artifacts**
   (kept for 14 days).

If you enter a `tag_name` on manual dispatch (e.g. `v0.3.1`), the workflow
sets the crate versions, builds, and creates a GitHub Release with that tag.

---

## Local build

Everything builds with cargo alone:

```bash
cargo build --release -p locagens-cli       # lcgn CLI
cargo build --release -p locagens-desktop   # locagens desktop app
```

Binaries land in `target/release/` (`lcgn` / `lcgn.exe`, `locagens` /
`locagens.exe`). Before tagging, run `cargo test --workspace`.

---

## Code signing

Not set up yet. macOS binaries are unsigned — Gatekeeper shows a warning on
first launch. When we have an Apple Developer certificate, it will be added
via repository secrets. Until then:

> "locagens" cannot be opened because the developer cannot be verified.

Fix: right-click (or Control-click) the app → **Open** → **Open** in the
dialog, or System Settings → Privacy & Security → **Open Anyway**.

---

## Auto-update

No auto-update support yet. Users download new versions manually from the
GitHub Releases page.

---

## Platform support

Release binaries: macOS (Apple Silicon + Intel) and Windows (x86_64 MSVC).
The code builds on Linux (CI) but no Linux release binary is published yet.
