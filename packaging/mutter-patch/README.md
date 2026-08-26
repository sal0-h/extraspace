# Mutter crash patch

ExtraSpace's sharp display path asks mutter for a virtual monitor at the
tablet's full panel resolution, passing the UI scale as the mode's
`preferred-scale`. On stock mutter 50.4 that logs you out.

## What goes wrong

Passing `modes` to `RecordVirtual` makes mutter reload the monitor
configuration. When the reload cannot apply a layout containing the new
monitor, the CRTC is left without a configuration — and two places then
dereference it anyway:

- `meta_virtual_monitor_get_crtc_mode()` reads `crtc_config->mode` without
  checking that a configuration exists.
- `meta_screen_cast_virtual_stream_src_get_specs()` uses the mode it returns,
  and `ensure_virtual_monitor()` compares against it.

Either one takes gnome-shell down with a SIGSEGV, which on Wayland means the
session ends and every open application dies with it.

`0001-virtual-monitor-guard-unconfigured-crtc.patch` adds the missing checks and
falls back to the mode that was requested, which is what the CRTC ends up using
anyway. It touches no ABI: the soname stays `libmutter-18.so=0-64`, so
gnome-shell does not need rebuilding.

`UPSTREAM-REPORT.md` is the write-up for GNOME, ready to paste into a new issue.
`upstream-main.patch` is the same fix ported to mutter `main`, where this code now
lives in `meta-stream-source-virtual.c`. If upstream takes the fix, none of this
directory is needed any more.

## Using it

```console
./build-and-install.sh   # builds and installs, then records the version
```

Then log out and back in — a running compositor keeps the library it started
with. `PKGBUILD` is Arch's `mutter` recipe with `pkgrel` bumped and the patch
added to `source`, so it tracks whatever version that recipe is based on.

To go back to the stock package:

```console
./rollback.sh
```

## Surviving mutter upgrades

A later `mutter` upgrade replaces the patched build with the stock one and the
crash comes back, so two things watch for it:

- `install-hook.sh` records the patched version under
  `$XDG_DATA_HOME/extraspace/patched-mutter` and installs a pacman hook that
  warns after any `mutter` transaction where the two disagree.
- ExtraSpace checks the same marker itself (`scaled_modes_allowed()`) and
  refuses the scaled path unless the installed version matches and the running
  shell is not still using a deleted `libmutter`. The result is a softer-looking
  tablet rather than a logout.

Rerun `build-and-install.sh` after an upgrade to get the sharp display back.
Pinning `mutter` with `IgnorePkg` is a worse trade: it is version-locked against
gnome-shell, so holding it back blocks GNOME upgrades entirely.
