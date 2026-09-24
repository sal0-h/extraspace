#!/bin/sh
# Reinstalls the stock Arch mutter package.
#
# Log out and back in afterwards. Remember that stock mutter crashes on
# XS_MUTTER_MODES=1, so drop that variable before logging back in.
set -eu

# Clearing the marker first means ExtraSpace refuses the scaled path from the
# next connect onwards, even if XS_MUTTER_MODES is still set in your environment.
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/extraspace/patched-mutter"

stock=$(ls -1 "$HOME/build/mutter-rollback"/mutter-*-x86_64.pkg.tar.zst 2>/dev/null | sort -V | tail -1 || true)
if [ -n "$stock" ]; then
  sudo pacman -U "$stock"
else
  sudo pacman -S mutter
fi
echo "Back on stock mutter. Log out and back in."
