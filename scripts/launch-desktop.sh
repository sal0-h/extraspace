#!/usr/bin/env bash
# Start the installed app. Native Wi-Fi discovery and saved pairing live inside
# Extraspace; ADB is only needed for USB and companion upgrades.
set -euo pipefail

BIN_DIR="$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")" && pwd)"

# Prefer the Android SDK client for USB companion installs when available.
for sdk_root in "${ANDROID_SDK_ROOT:-}" "${ANDROID_HOME:-}" "$HOME/Android/Sdk"; do
  if [[ -n "$sdk_root" && -x "$sdk_root/platform-tools/adb" ]]; then
    PATH="$sdk_root/platform-tools:$PATH"
    export PATH
    break
  fi
done

exec "$BIN_DIR/extraspace"
