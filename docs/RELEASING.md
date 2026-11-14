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

1. Bump the `version` fields in `apps/cli/Cargo.toml`,
   `apps/desktop-rs/Cargo.toml`, and `apps/cli/npm/package.json` (e.g. `0.2.0` →
   `0.3.0`), then refresh the lock file with `cargo check --workspace`.
2. Commit:
   ```bash
   git add apps/cli/Cargo.toml apps/desktop-rs/Cargo.toml apps/cli/npm/package.json Cargo.lock
   git commit -m "bump: v0.3.0"
   ```
3. Tag and push:
   ```bash
   git tag v0.3.0
   git push origin v0.3.0
   ```
4. GitHub Actions triggers **"Release (macOS & Windows binaries)"**
   (`.github/workflows/release-mac.yml`), which:
   - Sets the `version` of `apps/cli`, `apps/desktop-rs`, and
     `apps/cli/npm/package.json` from the tag (`.github/scripts/set-version.sh`),
     so the binaries and npm packages carry the tag version even if step 1 was
     skipped.
   - **macOS:** builds `aarch64` and `x86_64`, merges them into universal
     binaries, wraps the app as `Locagens.app` (icon from
     `apps/desktop-rs/assets/icon.png`, bundle id `com.locagens.app`), signs
     it and `lcgn` (universal + per-arch CLI binaries) with the Developer ID
     certificate (hardened runtime), notarizes and staples them, then builds,
     signs, notarizes and staples `Locagens-macOS-universal.dmg`. `lcgn` ships
     as `lcgn-macOS-universal.tar.gz`.
   - **Windows:** builds `locagens.exe` and `lcgn.exe` into
     `Locagens-Windows-x64.zip`, plus a CLI-only `lcgn-Windows-x64.zip`.
   - **npm (`lcgn`):** stages `lcgn-darwin-arm64`, `lcgn-darwin-x64`,
     `lcgn-win32-x64`, and the launcher package `lcgn` using
     `.github/scripts/stage-npm.sh`, packs `.tgz` files as the `locagens-npm`
     artifact, and publishes them to npm (`npm publish --access public`) on
     tagged releases when `NPM_TOKEN` is set.
   - Uploads everything as workflow artifacts and publishes a GitHub Release
     with auto-generated notes.

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

macOS signing and notarization use these repository secrets:

| Secret | Content |
| --- | --- |
| `MAC_CERTIFICATE_P12_BASE64` | Developer ID Application certificate + key (`.p12`, base64) |
| `MAC_CERTIFICATE_PASSWORD` | Export password of that `.p12` |
| `APPLE_API_KEY_ID` | App Store Connect API key id |
| `APPLE_API_ISSUER` | App Store Connect issuer id |
| `APPLE_API_KEY_P8` | The API key file (`AuthKey_*.p8`) contents |

Without them the workflow still builds, unsigned, with a warning; Gatekeeper
then asks on first launch (right-click → **Open**). Windows binaries are not
signed (SmartScreen may warn on first launch).

The release workflow never runs on branch pushes (those only run `ci.yml`:
build, clippy, tests). To check the full pipeline without publishing, use
Actions → "Release (macOS & Windows binaries)" → **Run workflow** with an
empty tag: it builds, signs and notarizes, and keeps the files as artifacts.
If Apple refuses notarization there (e.g. "A required agreement is missing
or has expired" — accept the latest agreements in the Apple Developer account
/ App Store Connect → Agreements), the build-only run still delivers a signed,
not notarized DMG with a warning; a tagged release fails instead.

---

## npm distribution (`lcgn`)

Users can install the CLI on macOS (Terminal) and Windows (PowerShell / CMD)
with a single command:

```bash
npm install -g lcgn
# or
pnpm add -g lcgn
```

The `lcgn` package uses `optionalDependencies` (`lcgn-darwin-arm64`,
`lcgn-darwin-x64`, `lcgn-win32-x64`) so `npm` and `pnpm` only download the native
binary for the user's OS/architecture without needing a `postinstall` script.
If optional dependencies are skipped, the launcher in `apps/cli/npm/bin/lcgn.js`
downloads the matching archive from GitHub Releases into `~/.locagens/bin/` on
first run.

### Publishing secret

Add a repository secret to enable automatic npm publishing on tagged releases:

| Secret | Content |
| --- | --- |
| `NPM_TOKEN` | npm Automation or Granular Access Token with publish permission (`lcgn`, `lcgn-darwin-arm64`, `lcgn-darwin-x64`, `lcgn-win32-x64`) |

If `NPM_TOKEN` is not set, the `npm` job still packs the `.tgz` tarballs into
the `locagens-npm` workflow artifact and emits a warning without failing the
release workflow.

---

## Auto-update

No auto-update support yet. Users download new versions manually from the
GitHub Releases page.

---

## Platform support

Release binaries: macOS (Apple Silicon + Intel) and Windows (x86_64 MSVC).
The code builds on Linux (CI) but no Linux release binary is published yet.
