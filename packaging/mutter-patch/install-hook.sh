#!/bin/sh
# Records the currently installed mutter as the patched one and installs the
# pacman hook that warns when a later upgrade replaces it.
#
# Split out from build-and-install.sh so it can be run on its own, without
# rebuilding mutter. Safe to re-run.
set -eu

cd "$(dirname "$0")"
here=$(pwd)

version=$(pacman -Q mutter | awk '{print $2}')
marker="${XDG_DATA_HOME:-$HOME/.local/share}/extraspace/patched-mutter"

# Refuse to vouch for a mutter this directory did not build, otherwise the
# marker would tell ExtraSpace that a stock package is safe to poke.
built=$(. ./PKGBUILD; echo "$pkgver-$pkgrel")
if [ "$version" != "$built" ]; then
  echo "installed mutter is $version but this tree builds $built; run build-and-install.sh first" >&2
  exit 1
fi

mkdir -p "$(dirname "$marker")"
printf '%s\n' "$version" > "$marker"
echo ">>> recorded $version as patched"

sed -e "s|__MARKER__|$marker|" -e "s|__BUILD__|$here/build-and-install.sh|" \
  extraspace-mutter-patch-check > /tmp/extraspace-mutter-patch-check
sudo install -Dm755 /tmp/extraspace-mutter-patch-check /usr/local/bin/extraspace-mutter-patch-check
sudo install -Dm644 98-extraspace-mutter-patch.hook /etc/pacman.d/hooks/98-extraspace-mutter-patch.hook
rm -f /tmp/extraspace-mutter-patch-check
echo ">>> pacman hook installed"
