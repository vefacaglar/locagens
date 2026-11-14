#!/usr/bin/env bash
# Stages (and optionally publishes) the `lcgn` npm package and its platform binary packages.
# Usage: bash .github/scripts/stage-npm.sh <version> <binaries-dir> <out-dir> [--publish]
set -euo pipefail

if [ "$#" -lt 3 ] || [ "$#" -gt 4 ]; then
  echo "Usage: $0 <version> <binaries-dir> <out-dir> [--publish]" >&2
  exit 1
fi

version="${1#v}"
binaries_dir="$2"
out_dir="$3"
publish_flag="${4:-}"

if [ -z "$version" ]; then
  echo "Error: version must not be empty" >&2
  exit 1
fi

if [ -n "$publish_flag" ] && [ "$publish_flag" != "--publish" ]; then
  echo "Error: unknown option '$publish_flag' (expected --publish)" >&2
  exit 1
fi

script_dir="$(cd "$(dirname "$0")" && pwd)"
repo_root="$(cd "$script_dir/../.." && pwd)"

bin_darwin_arm64="$binaries_dir/lcgn-darwin-arm64"
bin_darwin_x64="$binaries_dir/lcgn-darwin-x64"
bin_win32_x64="$binaries_dir/lcgn-win32-x64.exe"

for bin_path in "$bin_darwin_arm64" "$bin_darwin_x64" "$bin_win32_x64"; do
  if [ ! -f "$bin_path" ]; then
    echo "Error: missing required binary '$bin_path'" >&2
    exit 1
  fi
done

stage_platform_pkg() {
  local pkg_name="$1"
  local os_json="$2"
  local cpu_json="$3"
  local src_bin="$4"
  local bin_name="$5"
  local make_exec="$6"

  local pkg_dir="$out_dir/$pkg_name"
  rm -rf "$pkg_dir"
  mkdir -p "$pkg_dir/bin"
  cp "$src_bin" "$pkg_dir/bin/$bin_name"
  if [ "$make_exec" = "1" ]; then
    chmod 755 "$pkg_dir/bin/$bin_name"
  fi

  cat > "$pkg_dir/package.json" <<EOF
{
  "name": "$pkg_name",
  "version": "$version",
  "description": "Platform binary for lcgn ($pkg_name)",
  "license": "MIT",
  "repository": {
    "type": "git",
    "url": "https://github.com/vefacaglar/locagens.git"
  },
  "os": $os_json,
  "cpu": $cpu_json,
  "files": [
    "bin/$bin_name"
  ]
}
EOF
}

mkdir -p "$out_dir"

stage_platform_pkg "lcgn-darwin-arm64" '["darwin"]' '["arm64"]' "$bin_darwin_arm64" "lcgn" "1"
stage_platform_pkg "lcgn-darwin-x64" '["darwin"]' '["x64"]' "$bin_darwin_x64" "lcgn" "1"
stage_platform_pkg "lcgn-win32-x64" '["win32"]' '["x64", "arm64"]' "$bin_win32_x64" "lcgn.exe" "0"

main_dir="$out_dir/lcgn"
rm -rf "$main_dir"
mkdir -p "$main_dir/bin"
cp "$repo_root/apps/cli/npm/bin/lcgn.js" "$main_dir/bin/lcgn.js"
chmod 755 "$main_dir/bin/lcgn.js"
cp "$repo_root/README.md" "$main_dir/README.md"

awk -v v="$version" '
  /^  "version":[[:space:]]*"/ { sub(/:[[:space:]]*"[^"]+"/, ": \"" v "\"") }
  /^  "optionalDependencies":[[:space:]]*\{/ { in_opt=1 }
  in_opt && /^[[:space:]]*"[^"]+":[[:space:]]*"/ { sub(/:[[:space:]]*"[^"]+"/, ": \"" v "\"") }
  in_opt && /\}/ { in_opt=0 }
  { print }
' "$repo_root/apps/cli/npm/package.json" > "$main_dir/package.json"

echo "Staged npm packages ($version) in $out_dir"

if [ "$publish_flag" = "--publish" ]; then
  auth_token="${NODE_AUTH_TOKEN:-${NPM_TOKEN:-}}"
  if [ -z "$auth_token" ]; then
    echo "::warning::NPM_TOKEN secret is not set; skipping npm publish."
    exit 0
  fi
  export NODE_AUTH_TOKEN="$auth_token"
  for pkg in lcgn-darwin-arm64 lcgn-darwin-x64 lcgn-win32-x64 lcgn; do
    echo "Publishing $pkg@$version..."
    npm publish "$out_dir/$pkg" --access public
  done
fi
