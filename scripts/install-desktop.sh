#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")/.."

os="$(uname -s)"

case "$os" in
  Darwin)
    cargo build --release --locked -p locagens-desktop --bin locagens

    version="$(sed -nE 's/^version[[:space:]]*=[[:space:]]*"([^"]+)".*/\1/p' apps/desktop-rs/Cargo.toml | head -1)"
    version="${version:-0.2.0}"

    stage="$(mktemp -d)"
    trap 'rm -rf "$stage"' EXIT

    app="$stage/Locagens.app"
    mkdir -p "$app/Contents/MacOS" "$app/Contents/Resources"

    cp target/release/locagens "$app/Contents/MacOS/locagens"
    chmod 755 "$app/Contents/MacOS/locagens"

    if [ -f apps/desktop-rs/assets/icon.png ]; then
      iconset="$stage/AppIcon.iconset"
      mkdir -p "$iconset"
      for size in 16 32 128 256 512; do
        sips -z "$size" "$size" apps/desktop-rs/assets/icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
        double=$((size * 2))
        sips -z "$double" "$double" apps/desktop-rs/assets/icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
      done
      iconutil -c icns "$iconset" -o "$app/Contents/Resources/AppIcon.icns"
    fi

    cat > "$app/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
  <key>CFBundleName</key><string>Locagens</string>
  <key>CFBundleDisplayName</key><string>Locagens</string>
  <key>CFBundleIdentifier</key><string>com.locagens.app</string>
  <key>CFBundleExecutable</key><string>locagens</string>
  <key>CFBundleIconFile</key><string>AppIcon</string>
  <key>CFBundlePackageType</key><string>APPL</string>
  <key>CFBundleShortVersionString</key><string>${version}</string>
  <key>CFBundleVersion</key><string>${version}</string>
  <key>LSMinimumSystemVersion</key><string>11.0</string>
  <key>LSApplicationCategoryType</key><string>public.app-category.developer-tools</string>
  <key>NSHighResolutionCapable</key><true/>
</dict>
</plist>
PLIST
    plutil -lint "$app/Contents/Info.plist" >/dev/null

    codesign --force --deep --sign - "$app"

    if [ -n "${LOCAGENS_APP_DIR:-}" ]; then
      app_dir="$LOCAGENS_APP_DIR"
    elif [ -w /Applications ]; then
      app_dir="/Applications"
    else
      app_dir="$HOME/Applications"
    fi

    mkdir -p "$app_dir"
    target_app="$app_dir/Locagens.app"
    backup_app="$app_dir/Locagens.app.bak"
    rm -rf "$backup_app"
    if [ -e "$target_app" ]; then
      mv "$target_app" "$backup_app"
    fi
    if ! mv "$app" "$target_app"; then
      if [ -e "$backup_app" ]; then
        mv "$backup_app" "$target_app" || true
      fi
      exit 1
    fi
    rm -rf "$backup_app"

    echo "Installed: $target_app"
    ;;
  Linux)
    cargo install --path apps/desktop-rs --locked --force
    ;;
  *)
    echo "Unsupported OS: $os" >&2
    exit 1
    ;;
esac
