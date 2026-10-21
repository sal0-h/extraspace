#!/bin/sh
# Builds and installs the Mutter version in PKGBUILD with Extraspace's guards.
# Use --build-only to prepare a package, or --install-only to install it later.
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
mode=${1:-all}
case "$mode" in
  all|--build-only|--install-only) ;;
  *) echo "Usage: $0 [--build-only|--install-only]" >&2; exit 1 ;;
esac
if [ "$#" -gt 1 ]; then
  echo "Usage: $0 [--build-only|--install-only]" >&2
  exit 1
fi

check_installed_version() {
  # An old checkout must not replace an upgraded compositor with an older one.
  installed=$(pacman -Q mutter 2>/dev/null | awk '{print $2}' || true)
  if [ -n "$installed" ] && [ "$(vercmp "$version" "$installed")" -lt 0 ]; then
    echo "This recipe builds Mutter $version, older than installed $installed." >&2
    echo "Update the recipe and patches before rebuilding; refusing to downgrade." >&2
    exit 1
  fi
}
check_installed_version

if [ "$mode" != --install-only ]; then
  echo ">>> building mutter $version (this takes a while)"
  makepkg -sf
fi

package="$here/mutter-$version-x86_64.pkg.tar.zst"
if [ ! -f "$package" ]; then
  echo "Package not found: $package. Run $0 --build-only first." >&2
  exit 1
fi
if [ "$(pacman -Qp "$package")" != "mutter $version" ]; then
  echo "Package metadata does not match Mutter $version; refusing to install." >&2
  exit 1
fi

if [ "$mode" = --build-only ]; then
  echo "Package ready: $package"
  echo "Install it with: $here/build-and-install.sh --install-only"
  exit 0
fi

echo ">>> installing"
sudo -v
# The system may have upgraded while the package was being built.
check_installed_version
sudo pacman -U --needed "$package"

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
