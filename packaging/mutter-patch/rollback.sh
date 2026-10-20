#!/bin/sh
# Reinstalls the stock Arch mutter package.
#
# Log out and back in afterwards, then restart Extraspace. Without the patch
# marker, the app uses Standard image quality.
set -eu

# Remove the marker so Extraspace's compatibility check disables native scaled
# modes when the app next starts.
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/extraspace/patched-mutter"

stock=$(ls -1 "$HOME/build/mutter-rollback"/mutter-*-x86_64.pkg.tar.zst 2>/dev/null | sort -V | tail -1 || true)
if [ -n "$stock" ]; then
  sudo pacman -U "$stock"
else
  sudo pacman -S mutter
fi
echo "Back on stock mutter. Log out and back in."
