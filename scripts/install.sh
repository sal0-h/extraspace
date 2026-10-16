#!/usr/bin/env bash
# Installs Extraspace into your user account: binary, icon and desktop entry.
#
# Everything lands under ~/.local, so this needs no root and touches nothing
# system-wide. `./scripts/setup.sh` is the one that needs sudo, and it handles a
# different job -- system packages and the virtual camera device.
#
#   ./scripts/install.sh              # build if needed, then install
#   ./scripts/install.sh --uninstall  # remove everything this installed
set -euo pipefail

APP_ID="io.github.tymonoman.Extraspace"
BIN_NAME="extraspace"
EXTENSION_UUID="extraspace@sal0-h"

REPO_ROOT="$(cd "$(dirname "$(readlink -f "$0")")/.." && pwd)"
BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
DESKTOP_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor/scalable/apps"
EXTENSION_DIR="$DATA_DIR/gnome-shell/extensions/$EXTENSION_UUID"

bold() { printf '\033[1m%s\033[0m\n' "$*"; }
ok()   { printf '  \033[32m✓\033[0m %s\n' "$*"; }
warn() { printf '  \033[33m!\033[0m %s\n' "$*"; }

if [[ "${1:-}" == "--uninstall" ]]; then
  bold "Uninstalling Extraspace"
  rm -f "$BIN_DIR/$BIN_NAME" && ok "removed $BIN_DIR/$BIN_NAME"
  rm -f "$BIN_DIR/extraspace-launcher" && ok "removed desktop launcher helper"
  rm -f "$DESKTOP_DIR/$APP_ID.desktop" && ok "removed desktop entry"
  rm -f "$DESKTOP_DIR/$APP_ID.Fast.desktop"
  gnome-extensions disable "$EXTENSION_UUID" 2>/dev/null || true
  for extension_file in extension.js state.js metadata.json; do
    rm -f "$EXTENSION_DIR/$extension_file"
  done
  rmdir "$EXTENSION_DIR" 2>/dev/null || true
  rm -f "$HOME/.config/autostart/$APP_ID.desktop" && ok "removed autostart"
  rm -f "$ICON_DIR/$APP_ID.svg" && ok "removed icon"
  rm -f "$DATA_DIR/extraspace/extraspace.apk" && ok "removed companion APK"
  rmdir "$DATA_DIR/extraspace" 2>/dev/null || true
  update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
  gtk-update-icon-cache -f -t "$DATA_DIR/icons/hicolor" 2>/dev/null || true
  echo
  echo "Your settings in ~/.config/extraspace were left alone."
  exit 0
fi

bold "Installing Extraspace"
echo

# --- build -------------------------------------------------------------------
BINARY="$REPO_ROOT/target/release/$BIN_NAME"
if [[ ! -x "$BINARY" ]]; then
  warn "no release build found, building now (this takes a few minutes)"
  ( cd "$REPO_ROOT" && cargo build --release )
fi
[[ -x "$BINARY" ]] || { echo "build produced no binary at $BINARY"; exit 1; }

# --- binary ------------------------------------------------------------------
# Copied rather than symlinked: a symlink into target/ breaks the moment anyone
# runs `cargo clean`, and the failure ("No such file") gives no hint why.
mkdir -p "$BIN_DIR"
install -m755 "$BINARY" "$BIN_DIR/$BIN_NAME.new"
mv -f "$BIN_DIR/$BIN_NAME.new" "$BIN_DIR/$BIN_NAME"
ok "installed $BIN_DIR/$BIN_NAME"
install -m755 "$REPO_ROOT/scripts/launch-desktop.sh" "$BIN_DIR/extraspace-launcher"
ok "installed desktop launcher"

# --- companion APK -----------------------------------------------------------
# Desktop launches start with cwd=$HOME, so the relative android/ tree in the
# repo is invisible. Keep a copy next to the other user data so the host can
# still push upgrades.
SHARE_DIR="$DATA_DIR/extraspace"
APK_SRC=""
for candidate in \
  "$REPO_ROOT/android/app/build/outputs/apk/release/app-release.apk" \
  "$REPO_ROOT/android/app/build/outputs/apk/debug/app-debug.apk"
do
  if [[ -f "$candidate" ]]; then
    APK_SRC="$candidate"
    break
  fi
done
if [[ -n "$APK_SRC" ]]; then
  mkdir -p "$SHARE_DIR"
  install -m644 "$APK_SRC" "$SHARE_DIR/extraspace.apk"
  ok "installed $SHARE_DIR/extraspace.apk"
else
  warn "no companion APK in the tree; tablet upgrades will be skipped until you build one"
fi

# --- icon --------------------------------------------------------------------
mkdir -p "$ICON_DIR"
install -m644 "$REPO_ROOT/packaging/$APP_ID.svg" "$ICON_DIR/$APP_ID.svg"
ok "installed icon"

# --- desktop entry -----------------------------------------------------------
mkdir -p "$DESKTOP_DIR"
# Written out rather than copied so Exec points at the absolute installed path.
# GNOME will happily launch a bare `extraspace` only if ~/.local/bin is on PATH,
# and for a GUI launch that PATH comes from the session, not your shell rc --
# so a relative Exec silently fails to start for some users.
cat > "$DESKTOP_DIR/$APP_ID.desktop" <<EOF
[Desktop Entry]
Type=Application
Name=Extraspace
GenericName=Tablet Display
Comment=Use an Android tablet as an extra display and webcam
Exec=$BIN_DIR/extraspace-launcher
Icon=$APP_ID
Terminal=false
Categories=Utility;GTK;GNOME;
Keywords=display;monitor;tablet;android;screen;webcam;camera;second screen;
StartupNotify=true
StartupWMClass=$BIN_NAME
X-GNOME-Autostart-enabled=false
Actions=Background;Settings;Pair;

[Desktop Action Background]
Name=Start tablet display
Exec=$BIN_DIR/extraspace-launcher --background

[Desktop Action Settings]
Name=Settings
Exec=$BIN_DIR/extraspace-launcher --settings

[Desktop Action Pair]
Name=Pair tablet
Exec=$BIN_DIR/extraspace-launcher --pair
EOF
ok "installed desktop entry"
# Earlier personal installations used a second entry with the same app name.
# Keep one search result, including its new background and settings actions.
if [[ -f "$DESKTOP_DIR/$APP_ID.Fast.desktop" ]] && \
   grep -Fq "Exec=$BIN_DIR/extraspace-launcher" "$DESKTOP_DIR/$APP_ID.Fast.desktop"; then
  rm -f "$DESKTOP_DIR/$APP_ID.Fast.desktop"
fi

mkdir -p "$EXTENSION_DIR"
for extension_file in extension.js state.js metadata.json; do
  install -m644 "$REPO_ROOT/extension/$extension_file" "$EXTENSION_DIR/$extension_file"
done
ok "installed Quick Settings extension"
if command -v gnome-extensions >/dev/null && [[ "$(gnome-shell --version 2>/dev/null)" == "GNOME Shell 51."* ]]; then
  if gnome-extensions enable "$EXTENSION_UUID" 2>/dev/null; then
    ok "enabled Extraspace in Quick Settings"
  else
    # Shell may not discover a brand-new extension until its next login. Save
    # the choice now without replacing any of the user's other extensions.
    if command -v gjs >/dev/null; then
      gjs -c 'const Gio = imports.gi.Gio; const s = new Gio.Settings({schema_id: "org.gnome.shell"}); const enabled = s.get_strv("enabled-extensions"); if (!enabled.includes("extraspace@sal0-h")) s.set_strv("enabled-extensions", [...enabled, "extraspace@sal0-h"]); s.set_strv("disabled-extensions", s.get_strv("disabled-extensions").filter(id => id !== "extraspace@sal0-h")); Gio.Settings.sync();'
    fi
    warn "Log out and back in once to load the new Quick Settings extension."
  fi
fi

# The Shell tile is available at login; the streaming process starts only when
# requested. Clear any old autostart entry.
rm -f "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/$APP_ID.desktop"

update-desktop-database "$DESKTOP_DIR" 2>/dev/null || true
gtk-update-icon-cache -f -t "$DATA_DIR/icons/hicolor" 2>/dev/null || true

# --- PATH check --------------------------------------------------------------
echo
case ":$PATH:" in
  *":$BIN_DIR:"*) ok "$BIN_DIR is on your PATH" ;;
  *) warn "$BIN_DIR is not on your PATH, so \`$BIN_NAME\` will not work in a terminal."
     warn "The app grid launcher works regardless. To fix the terminal case:"
     echo "      echo 'export PATH=\"\$HOME/.local/bin:\$PATH\"' >> ~/.bashrc" ;;
esac

echo
bold "Done."
echo "Find \"Extraspace\" in your applications, or run: $BIN_NAME"
echo "Remove it again with: ./scripts/install.sh --uninstall"
