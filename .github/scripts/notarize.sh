#!/usr/bin/env bash
# Submits a zip or DMG to Apple's notary service with the App Store Connect
# API key and waits; prints the notary log and fails unless it is accepted.
# Failures are also emitted as workflow error annotations.
set -uo pipefail
file="$1"
auth=(--key "$APPLE_API_KEY_PATH" --key-id "$APPLE_API_KEY_ID" --issuer "$APPLE_API_ISSUER")
result=$(xcrun notarytool submit "$file" "${auth[@]}" --wait --timeout 30m --output-format json 2>&1)
code=$?
echo "$result"
if [ $code -ne 0 ]; then
  echo "::error::notarytool submit failed ($code): $(printf '%s' "$result" | tr '\n' ' ' | cut -c1-900)"
  exit 1
fi
id=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("id",""))' 2>/dev/null || true)
status=$(printf '%s' "$result" | /usr/bin/python3 -c 'import json,sys; print(json.load(sys.stdin).get("status",""))' 2>/dev/null || true)
if [ "$status" != "Accepted" ]; then
  log=""
  [ -n "$id" ] && log=$(xcrun notarytool log "$id" "${auth[@]}" 2>&1 || true)
  echo "$log"
  echo "::error::Notarization of $file ended with status: ${status:-unknown}. $(printf '%s' "$log" | tr '\n' ' ' | cut -c1-900)"
  exit 1
fi
