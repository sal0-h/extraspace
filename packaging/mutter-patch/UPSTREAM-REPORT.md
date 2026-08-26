# gnome-shell SIGSEGV: screen-cast virtual monitor dereferences an unassigned CRTC config

File at https://gitlab.gnome.org/GNOME/mutter/-/issues (component: mutter, label: screen-cast).

## Summary

Any client of `org.gnome.Mutter.ScreenCast.Session.RecordVirtual` can crash
gnome-shell, which on a Wayland session logs the user out and loses their
running applications. There are two distinct NULL dereferences, and every way of
sizing a virtual monitor hits one of them, so a third-party client cannot avoid
this by calling the API differently.

Both traces bottom out in the same place: `meta_virtual_monitor_get_crtc_mode()`
returns `crtc_config->mode` without checking whether the CRTC has a
configuration yet.

## Version

- mutter 50.4 (`8fe247a`), Arch Linux, native KMS backend, single laptop panel (2304x1440).
- The same code is present on `gnome-50` HEAD, and on `main` after the
  `MetaStream` extraction in `b35127b` (`meta-stream-source-virtual.c`).

## Crash 1 -- `Start`, when `modes` is passed

`RecordVirtual` with a `modes` array sets `mode_infos`, so `get_specs()` reads
the CRTC's assigned mode. `initable_init()` creates the virtual monitor and calls
`meta_monitor_manager_reload()` immediately before this, but if that reload has
not assigned a configuration to the new CRTC, `crtc_config` is NULL:

```
meta_screen_cast_virtual_stream_src_get_specs      <-- SIGSEGV
build_format_params
meta_screen_cast_stream_src_initable_init
g_initable_new
meta_screen_cast_virtual_stream_create_src
meta_screen_cast_session_start
handle_start
_meta_dbus_remote_desktop_session_skeleton_handle_method_call
```

`src/backends/meta-screen-cast-virtual-stream-src.c`:

```c
  if (!virtual_src->mode_infos)
    return FALSE;

  crtc_mode = meta_virtual_monitor_get_crtc_mode (virtual_src->virtual_monitor);
  crtc_mode_info = meta_crtc_mode_get_info (crtc_mode);   /* crtc_mode is NULL */
```

This is intermittent with an identical mode list: two sessions came up fine and
the next two killed the shell.

## Crash 2 -- format renegotiation, when `modes` is omitted

Without `modes`, `mode_infos` is NULL, `get_specs()` returns FALSE, and mutter
sizes the monitor from PipeWire instead. `notify_params_updated()` then runs
`ensure_virtual_monitor()` from inside the format callback, which dereferences
the same pointer once `virtual_monitor` exists:

```
meta_screen_cast_virtual_stream_src_notify_params_updated   <-- SIGSEGV
on_format_param_changed
pw_impl_port_set_param
...
pipewire_loop_source_dispatch
```

The first format update creates the monitor and calls
`meta_monitor_manager_reload()`; the reload emits `monitors-changed-internal`,
whose handler renegotiates, producing a second format update while the CRTC
configuration may still be unassigned.

## Reproducer

1. `RemoteDesktop.CreateSession`, `ScreenCast.CreateSession` with
   `remote-desktop-session-id`, `ScreenCast.Session.RecordVirtual` with
   `{"is-platform": true, "cursor-mode": 2, "modes": [{"size": (2296, 1428), "refresh-rate": 60.0, "is-preferred": true, "preferred-scale": 1.75}]}`.
2. `RemoteDesktop.Session.Start` -- crash 1 fires here, intermittently.
3. Omit `modes`, connect a PipeWire consumer with a fixed `SPA_PARAM_EnumFormat`
   rectangle, and let mutter renegotiate -- crash 2 fires from the format callback.

Having stale `Meta-0` layouts in `monitors.xml` makes it much easier to hit:
virtual-monitor serials restart at `0x000001` per gnome-shell process, so a saved
layout for an earlier virtual monitor matches a new one and pins a mode. In one
session mutter reported a saved 1316x822 mode for a monitor created with a single
2296x1428 mode.

## Suggested fix

Guard the accessor and give both callers something sensible to do:

```diff
--- a/src/backends/meta-virtual-monitor.c
+++ b/src/backends/meta-virtual-monitor.c
@@ -154,6 +154,9 @@ meta_virtual_monitor_get_crtc_mode (MetaVirtualMonitor *virtual_monitor)
   crtc_config = meta_crtc_get_config (priv->crtc);
+  if (!crtc_config)
+    return NULL;
+
   return crtc_config->mode;
```

In `get_specs()`, fall back to the requested mode (the head of `mode_infos` is
the preferred one), which is the mode the CRTC ends up using anyway. In
`ensure_virtual_monitor()`, treat a NULL current mode as "no mode to compare
against" and set the modes unconditionally. Full patch:
`0001-virtual-monitor-guard-unconfigured-crtc.patch`.

## Related commits

- `d1ad3a388577` "virtual-monitor: Get CRTC mode directly from CRTC" -- replaced
  the always-set `priv->crtc_mode` with an unguarded `crtc_config->mode`.
- `61179d1032f2` "screen-cast: Allow creating non-resizable virtual monitors" --
  made `get_specs()` read the live CRTC when `mode_infos` is set.
