# Mutter virtual-monitor patches

Extraspace's Sharper mode requests a near-native virtual monitor with a GNOME
logical scale. The supplied Arch package targets Mutter 51.1-2 and GNOME Shell
51. It includes guards for crashes reproduced on unpatched Mutter 50.4, 50.5,
and 51.0. This is an Arch-specific build; it does not configure other
packaging systems.

## Patches

`0001-virtual-monitor-guard-unconfigured-crtc.patch` handles a virtual CRTC
without an assigned configuration. It guards the mode accessor and its callers,
using the requested mode for initial stream specifications and skipping the
current-mode comparison when none exists.

`0002-virtual-monitor-skip-null-stage-view.patch` guards cursor update paths
while the virtual monitor has no renderer view. Without these checks, creating
a virtual monitor can crash GNOME Shell and end the Wayland session.

The package keeps `libmutter-51.so`, so the matching GNOME Shell does not need
rebuilding. The [upstream report](UPSTREAM-REPORT.md) records the original CRTC
crash. `upstream-main.patch` and `upstream-main-null-view.patch` contain the
corresponding ports to the upstream revision used during that work.

## Build and install

From the repository root:

```bash
./packaging/mutter-patch/build-and-install.sh
```

The script builds with `makepkg`, installs with `sudo pacman -U`, records the
patched package version, and installs a pacman warning hook. Log out and back
in afterward so GNOME Shell loads the new library.

Open Extraspace normally and select **Image quality > Sharper**. No environment
flag is required. The compatibility check must pass before the app uses it.

To build without installing, use `--build-only`. Once the package is ready,
`--install-only` installs it without recompiling. The installer verifies the
package version and refuses to downgrade a newer installed Mutter.

## Upgrades

A system upgrade can replace the patched library. The pacman hook compares the
installed package version with the marker at
`$XDG_DATA_HOME/extraspace/patched-mutter` (default
`~/.local/share/extraspace/patched-mutter`) and warns if they differ.

Extraspace's `scaled_modes_supported()` checks that marker against the local
pacman database and checks for a replaced library still mapped by GNOME Shell.
If the check fails, the app uses Standard. After an upgrade, update the package
recipe and patches for the new Mutter version, rebuild, then log out/in and
restart Extraspace. Holding Mutter back can conflict with GNOME Shell upgrades.

## Rollback

```bash
./packaging/mutter-patch/rollback.sh
```

This removes the marker and reinstalls stock Mutter, preferring a cached package
under `~/build/mutter-rollback` if present. Log out/in and restart Extraspace
afterward. The app will use Standard until a matching patched build is restored.
