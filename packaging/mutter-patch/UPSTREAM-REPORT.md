# Upstream bug report (ready to paste)

File this at https://gitlab.gnome.org/GNOME/mutter/-/issues/new — everything from
"Affected version" down is the issue body.

Title:

    Screen-cast virtual monitor dereferences an unassigned CRTC config, crashing gnome-shell

Suggested labels: `1. Crash`, `screen cast`

---

### Affected version

* Arch Linux, kernel 7.1.9, Wayland session, native KMS backend, single laptop panel (2304x1440).
* mutter 50.4.
* Still present on `main` at `82ad6279` (2026-08-25). The code moved in the
  `MetaStream` extraction, so the three unguarded sites are now:
  * `src/backends/meta-virtual-monitor.c:171` — `meta_virtual_monitor_get_crtc_mode()`
  * `src/backends/meta-stream-source-virtual.c:116` — `meta_stream_source_virtual_get_specs()`
  * `src/backends/meta-stream-source-virtual.c:716` — `ensure_virtual_monitor()`

### Bug summary

Any client of `org.gnome.Mutter.ScreenCast.Session.RecordVirtual` can crash
gnome-shell, which on a Wayland session logs the user out and loses their running
applications.

There are two distinct NULL dereferences, one on each side of a branch, so a
third-party client cannot avoid the problem by calling the API differently:
passing `modes` to `RecordVirtual` can crash at `Start`, and omitting `modes`
can crash during PipeWire format renegotiation.

Both bottom out in the same accessor. `meta_virtual_monitor_get_crtc_mode()`
returns `crtc_config->mode` without checking whether the monitor manager has
assigned a configuration to the CRTC yet:

```c
  crtc_config = meta_crtc_get_config (priv->crtc);
  return crtc_config->mode;
```

Both callers then feed the result straight into `meta_crtc_mode_get_info()`.

### Steps to reproduce

1. `RemoteDesktop.CreateSession`, then `ScreenCast.CreateSession` with that
   session's `remote-desktop-session-id`.
2. `ScreenCast.Session.RecordVirtual` with
   `{"is-platform": true, "cursor-mode": 2, "modes": [{"size": (2296, 1428), "refresh-rate": 60.0, "is-preferred": true, "preferred-scale": 1.75}]}`.
3. `RemoteDesktop.Session.Start` — crash 1 fires here, intermittently.
4. For crash 2, omit `modes`, connect a PipeWire consumer that advertises a fixed
   `SPA_PARAM_EnumFormat` rectangle, and let mutter renegotiate.

Stale `Meta-0` layouts in `monitors.xml` make this much easier to hit.
Virtual-monitor serials restart at `0x000001` in every gnome-shell process, so a
saved layout from an earlier virtual monitor matches a new one and pins a mode.
In one session mutter reported a saved 1316x822 mode for a monitor that was
created with a single 2296x1428 mode.

### What happened

gnome-shell dies with SIGSEGV and the Wayland session is torn down.

### What did you expect to happen

A screen-cast client should not be able to crash the compositor by asking for a
virtual monitor, whatever mode list it supplies.

### Relevant logs

Crash 1 — `Start`, with `modes` passed. `mode_infos` is set, so `get_specs()`
reads the CRTC's assigned mode. `initable_init()` creates the virtual monitor and
calls `meta_monitor_manager_reload()` just before this, but when that reload does
not assign a configuration to the new CRTC, `crtc_config` is NULL:

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

This is intermittent with an identical mode list: two sessions came up fine and
the next two killed the shell.

Crash 2 — format renegotiation, with `modes` omitted. `mode_infos` is NULL,
`get_specs()` returns FALSE, and mutter sizes the monitor from PipeWire instead.
`notify_params_updated()` runs `ensure_virtual_monitor()` from inside the format
callback, which dereferences the same pointer once `virtual_monitor` exists:

```
meta_screen_cast_virtual_stream_src_notify_params_updated   <-- SIGSEGV
on_format_param_changed
pw_impl_port_set_param
...
pipewire_loop_source_dispatch
```

The first format update creates the monitor and calls
`meta_monitor_manager_reload()`; that reload emits `monitors-changed-internal`,
whose handler renegotiates, producing a second format update while the CRTC
configuration may still be unassigned.

### Suggested fix

Guard the accessor and give both callers something sensible to do when there is
no configuration yet. In `get_specs()`, fall back to the requested mode — the
head of `mode_infos` is the preferred one, and it is the mode the CRTC ends up
using anyway. In `ensure_virtual_monitor()`, treat a NULL current mode as "no
mode to compare against" and set the modes unconditionally.

The patch below applies cleanly to `main` at `82ad6279`. I am running the
equivalent patch against 50.4: before it, the crashes were frequent enough to log
me out several times in an afternoon, and since it I have not reproduced either
one across many session create/teardown cycles, with virtual monitors otherwise
behaving normally.

```diff
--- a/src/backends/meta-virtual-monitor.c
+++ b/src/backends/meta-virtual-monitor.c
@@ -169,6 +169,9 @@
   const MetaCrtcConfig *crtc_config;
 
   crtc_config = meta_crtc_get_config (priv->crtc);
+  if (!crtc_config)
+    return NULL;
+
   return crtc_config->mode;
 }
 
--- a/src/backends/meta-stream-source-virtual.c
+++ b/src/backends/meta-stream-source-virtual.c
@@ -114,6 +114,21 @@
     return FALSE;
 
   crtc_mode = meta_virtual_monitor_get_crtc_mode (source_virtual->virtual_monitor);
+  if (!crtc_mode)
+    {
+      /* The monitor manager has not assigned a CRTC configuration yet, which
+       * happens when the reload in initable_init() could not apply a layout
+       * containing this monitor. Fall back to the mode that was requested,
+       * which is what the CRTC will end up using anyway.
+       */
+      const MetaVirtualModeInfo *mode_info = source_virtual->mode_infos->data;
+
+      *width = mode_info->width;
+      *height = mode_info->height;
+      *frame_rate = mode_info->refresh_rate;
+      return TRUE;
+    }
+
   crtc_mode_info = meta_crtc_mode_get_info (crtc_mode);
 
   *width = crtc_mode_info->width;
@@ -715,12 +730,21 @@
       MetaCrtcMode *crtc_mode =
         meta_virtual_monitor_get_crtc_mode (virtual_monitor);
       g_autolist (MetaVirtualModeInfo) mode_infos = NULL;
-      const MetaCrtcModeInfo *mode_info = meta_crtc_mode_get_info (crtc_mode);
 
-      if (mode_info->width == video_format->size.width &&
-          mode_info->height == video_format->size.height &&
-          mode_info->preferred_scale == source_virtual->preferred_scale)
-        return;
+      /* A CRTC configuration may not have been assigned yet, in which case
+       * there is no current mode to compare against and the modes below are
+       * set unconditionally.
+       */
+      if (crtc_mode)
+        {
+          const MetaCrtcModeInfo *mode_info =
+            meta_crtc_mode_get_info (crtc_mode);
+
+          if (mode_info->width == video_format->size.width &&
+              mode_info->height == video_format->size.height &&
+              mode_info->preferred_scale == source_virtual->preferred_scale)
+            return;
+        }
 
       mode_infos = g_list_append (mode_infos, create_mode_info (source_virtual,
                                                                 video_format));
```

I am happy to open this as a merge request if that is easier to review.

### Related

* `d1ad3a38` "virtual-monitor: Get CRTC mode directly from CRTC" replaced the
  always-set `priv->crtc_mode` with the unguarded `crtc_config->mode`.
* `61179d10` "screen-cast: Allow creating non-resizable virtual monitors" made
  `get_specs()` read the live CRTC whenever `mode_infos` is set.
* #4314 was a nearby screen-cast virtual monitor crash, since fixed.
* #4839 describes reloads that do not apply the requested layout, which is one
  way the CRTC ends up without a configuration.
