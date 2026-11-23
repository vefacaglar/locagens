#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

cargo build --release --locked -p locagens-cli --bin lcgn

if [ -n "${LCGN_INSTALL_DIR:-}" ]; then
  dest="$LCGN_INSTALL_DIR"
elif case ":$PATH:" in *":$HOME/.local/bin:"*) true ;; *) false ;; esac || [ -e "$HOME/.npm-global/bin/lcgn" ]; then
  dest="$HOME/.local/bin"
else
  dest="$HOME/.cargo/bin"
fi

mkdir -p "$dest"

tmp="$(mktemp "$dest/.lcgn.XXXXXX")"
trap 'rm -f "$tmp"' EXIT

cp target/release/lcgn "$tmp"
chmod 755 "$tmp"

if [ "$(uname -s)" = "Darwin" ]; then
  codesign --force --sign - "$tmp"
fi

mv -f "$tmp" "$dest/lcgn"
trap - EXIT

echo "Installed: $dest/lcgn"
"$dest/lcgn" --version

resolved="$(command -v lcgn 2>/dev/null || true)"
if [ -z "$resolved" ]; then
  echo "Warning: $dest/lcgn is not on PATH. Add $dest to your PATH." >&2
elif [ "$resolved" != "$dest/lcgn" ]; then
  echo "Warning: 'lcgn' on PATH resolves to $resolved, which shadows $dest/lcgn" >&2
fi
