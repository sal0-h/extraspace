#!/bin/sh
# Builds mutter 51.0-2 (51.0 plus the Extraspace virtual-monitor crash guards)
# and installs it.
#
# Needs sudo twice: once for makepkg to pull in build dependencies, once for
# pacman to install the result. Nothing else on the system is touched -- the
# soname stays libmutter-51.so, matching gnome-shell 51.
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

Open Extraspace normally and choose Image quality > Sharper in Settings.
No environment flag is needed.

A later Mutter upgrade can replace the patched library. Extraspace will use
Standard when its compatibility check fails. Update this package recipe and
patches for the new version, rebuild, then log out and back in.

To go back to the stock package at any time:

    $here/rollback.sh
EOF
