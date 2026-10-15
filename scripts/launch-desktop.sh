#!/usr/bin/env bash
# Start Extraspace with the paired wireless tablet when ADB has discovered it.
# If no wireless device appears, the app keeps its normal first-ready-device
# behavior, which makes USB a seamless fallback.
set -euo pipefail

BIN_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# Prefer the full Android SDK client when it is installed. It provides the mDNS
# discovery needed to reconnect to Android's paired wireless-debugging service.
for sdk_root in "${ANDROID_SDK_ROOT:-}" "${ANDROID_HOME:-}" "$HOME/Android/Sdk"; do
  if [[ -n "$sdk_root" && -x "$sdk_root/platform-tools/adb" ]]; then
    PATH="$sdk_root/platform-tools:$PATH"
    export PATH
    break
  fi
done

if [[ -z "${ANDROID_SERIAL:-}" ]] && command -v adb >/dev/null 2>&1; then
  adb start-server >/dev/null 2>&1 || true

  # mDNS discovery can lag behind login or waking the tablet. Give it a moment
  # to publish the paired Wi-Fi connection, and prefer that over USB if present.
  for _ in {1..6}; do
    wireless_serial="$(adb devices 2>/dev/null | awk '
      NR > 1 && $2 == "device" &&
      $1 ~ /^[0-9]+(\.[0-9]+){3}:[0-9]+$/ { print $1; exit }
    ')"
    if [[ -n "$wireless_serial" ]]; then
      export ANDROID_SERIAL="$wireless_serial"
      break
    fi
    sleep 0.5
  done
fi

exec "$BIN_DIR/extraspace"
