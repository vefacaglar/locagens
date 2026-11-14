#!/usr/bin/env bash
# Sets the version of the two released crates from a release tag
# (`v1.2.3` or `1.2.3`). Without a version-like tag nothing changes.
set -euo pipefail
tag="${1:-}"
version="${tag#v}"
case "$version" in
  [0-9]*) ;;
  *) echo "No release tag; keeping crate versions."; exit 0 ;;
esac
for cargo_toml in apps/cli/Cargo.toml apps/desktop-rs/Cargo.toml; do
  # Only the first `version = "..."` line (the package's own).
  awk -v v="$version" '!done && /^version[[:space:]]*=/ { print "version = \"" v "\""; done=1; next } { print }' \
    "$cargo_toml" > "$cargo_toml.tmp"
  mv "$cargo_toml.tmp" "$cargo_toml"
  echo "Set $cargo_toml to $version"
done
npm_pkg="apps/cli/npm/package.json"
if [ -f "$npm_pkg" ]; then
  awk -v v="$version" '
    /^  "version":[[:space:]]*"/ { sub(/:[[:space:]]*"[^"]+"/, ": \"" v "\"") }
    /^  "optionalDependencies":[[:space:]]*\{/ { in_opt=1 }
    in_opt && /^[[:space:]]*"[^"]+":[[:space:]]*"/ { sub(/:[[:space:]]*"[^"]+"/, ": \"" v "\"") }
    in_opt && /\}/ { in_opt=0 }
    { print }
  ' "$npm_pkg" > "$npm_pkg.tmp"
  mv "$npm_pkg.tmp" "$npm_pkg"
  echo "Set $npm_pkg to $version"
fi
# Keep Cargo.lock in step so `--locked` builds still pass.
if command -v cargo >/dev/null 2>&1; then
  cargo update --workspace --offline >/dev/null 2>&1 || cargo update --workspace
fi
