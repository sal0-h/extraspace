#!/bin/sh
# Reinstalls the stock Arch mutter 50.4-1, downloaded before patching.
#
# Log out and back in afterwards. Remember that stock 50.4 crashes on
# XS_MUTTER_MODES=1, so drop that variable before logging back in.
set -eu

# Clearing the marker first means ExtraSpace refuses the scaled path from the
# next connect onwards, even if XS_MUTTER_MODES is still set in your environment.
rm -f "${XDG_DATA_HOME:-$HOME/.local/share}/extraspace/patched-mutter"

sudo pacman -U "$HOME/build/mutter-rollback/mutter-50.4-1-x86_64.pkg.tar.zst"
echo "Back on stock mutter. Log out and back in."
