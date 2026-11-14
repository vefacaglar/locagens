#!/usr/bin/env bash
# Submits a zip or DMG to Apple's notary service with the App Store Connect
# API key and waits; prints the notary log and fails unless it is accepted.
set -euo pipefail
file="$1"
result=$(xcrun notarytool submit "$file" \
  --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER" \
  --wait --timeout 30m --output-format json)
echo "$result"
id=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))')
status=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))')
if [ "$status" != "Accepted" ]; then
  [ -n "$id" ] && xcrun notarytool log "$id" \
    --key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER" || true
  echo "::error::Notarization of $file ended with status: ${status:-unknown}"
  exit 1
fi
