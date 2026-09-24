#!/bin/sh
# Builds mutter 50.5-2 (50.5 plus the ExtraSpace virtual-monitor crash guards)
# and installs it.
#
# Needs sudo twice: once for makepkg to pull in build dependencies, once for
# pacman to install the result. Nothing else on the system is touched -- the
# soname stays libmutter-18.so=0-64, so gnome-shell does not need rebuilding.
#
# The new library is only picked up by a freshly started compositor, so log out
# and back in afterwards.
set -eu

cd "$(dirname "$0")"

here=$(pwd)
version=$(. ./PKGBUILD; echo "$pkgver-$pkgrel")

echo ">>> building mutter $version (this takes a while)"
makepkg -sf

echo ">>> installing"
sudo pacman -U --needed "mutter-$version-x86_64.pkg.tar.zst"

"$here/install-hook.sh"

cat <<EOF

Done. Log out and back in, then:

    pacman -Q mutter          # expect: mutter $version

To get the sharp, panel-resolution display, run ExtraSpace with:

    XS_MUTTER_MODES=1 extraspace

A later mutter upgrade will print a warning and ExtraSpace will fall back to the
safe sizing on its own, so that variable is harmless to leave set. Rerun this
script to patch the new version.

To go back to the stock package at any time:

    $here/rollback.sh
EOF
