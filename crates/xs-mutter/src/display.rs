//! Read mutter's current monitor list for the mirror-source picker.
//!
//! Do not call ApplyMonitorsConfig from this crate. On mutter 50.4 that races
//! PipeWire format negotiation (`notify_params_updated` → NULL CRTC) and
//! SIGSEGVs gnome-shell.

use std::collections::HashMap;

use tracing::warn;
use zbus::Connection;
use zvariant::OwnedValue;

use crate::{Error, Result};

type MonitorSpec = (String, String, String, String);
type Mode = (
    String,
    i32,
    i32,
    f64,
    f64,
    Vec<f64>,
    HashMap<String, OwnedValue>,
);
type Monitor = (MonitorSpec, Vec<Mode>, HashMap<String, OwnedValue>);
type LogicalMonitor = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<MonitorSpec>,
    HashMap<String, OwnedValue>,
);

/// Connector names mutter currently knows about (e.g. `["DP-3", "HDMI-1"]`).
pub async fn list_connectors(conn: &Connection) -> Result<Vec<String>> {
    let reply = conn
        .call_method(
            Some("org.gnome.Mutter.DisplayConfig"),
            "/org/gnome/Mutter/DisplayConfig",
            Some("org.gnome.Mutter.DisplayConfig"),
            "GetCurrentState",
            &(),
        )
        .await?;
    let (_serial, monitors, _logical, _properties): (
        u32,
        Vec<Monitor>,
        Vec<LogicalMonitor>,
        HashMap<String, OwnedValue>,
    ) = reply.body().deserialize().map_err(|e| {
        Error::DBus(zbus::Error::Failure(format!(
            "GetCurrentState deserialize: {e}"
        )))
    })?;
    Ok(monitors.into_iter().map(|(spec, ..)| spec.0).collect())
}

/// What mutter configured for the virtual monitor.
#[derive(Debug, Clone, Copy)]
pub struct VirtualState {
    pub width: u32,
    pub height: u32,
    /// Logical scale of the monitor's layout, i.e. how big the UI actually is.
    pub scale: f64,
}

/// Reads back the virtual monitor's current mode and logical scale.
///
/// `GetCurrentState` only reads, so it cannot disturb the screen-cast the way
/// `ApplyMonitorsConfig` does.
pub async fn virtual_monitor_state(conn: &Connection) -> Option<VirtualState> {
    let reply = conn
        .call_method(
            Some("org.gnome.Mutter.DisplayConfig"),
            "/org/gnome/Mutter/DisplayConfig",
            Some("org.gnome.Mutter.DisplayConfig"),
            "GetCurrentState",
            &(),
        )
        .await
        .ok()?;
    let (_serial, monitors, logical, _properties): (
        u32,
        Vec<Monitor>,
        Vec<LogicalMonitor>,
        HashMap<String, OwnedValue>,
    ) = reply.body().deserialize().ok()?;

    let is_virtual = |spec: &MonitorSpec| spec.1 == "MetaVendor";
    let (spec, modes, _props) = monitors.into_iter().find(|(spec, ..)| is_virtual(spec))?;
    let (_id, width, height, ..) = modes.into_iter().find(|(_, _, _, _, _, _, props)| {
        props
            .get("is-current")
            .and_then(|v| bool::try_from(v.clone()).ok())
            .unwrap_or(false)
    })?;
    let scale = logical
        .iter()
        .find(|(.., specs, _)| specs.iter().any(|s| s.0 == spec.0))
        .map(|(_, _, scale, ..)| *scale)
        .unwrap_or(1.0);

    Some(VirtualState {
        width: width.max(0) as u32,
        height: height.max(0) as u32,
        scale,
    })
}

/// Drops saved layouts that mention a virtual monitor, keeping a `.bak` once.
///
/// mutter reuses virtual-monitor serials (`0x000001` upward, per gnome-shell
/// process), so a layout saved for an *older* Meta-0 mode matches a new one. If
/// that pinned mode is not in the mode list we pass, the CRTC is left
/// unconfigured and `meta_screen_cast_virtual_stream_src_get_specs` dereferences
/// it -- gnome-shell SIGSEGVs and the user is logged out. Editing the file only
/// touches our own leftovers; real monitors' layouts are kept.
pub fn purge_saved_virtual_layouts() -> usize {
    let Some(path) = dirs_config_dir().map(|d| d.join("monitors.xml")) else {
        return 0;
    };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return 0;
    };

    let mut kept = String::with_capacity(text.len());
    let mut removed = 0;
    let mut rest = text.as_str();
    while let Some(start) = rest.find("<configuration>") {
        let Some(end_rel) = rest[start..].find("</configuration>") else {
            break;
        };
        let end = start + end_rel + "</configuration>".len();
        let block = &rest[start..end];
        if block.contains("MetaVendor") || block.contains("Virtual remote monitor") {
            // Drop the block and the blank line it sat on.
            kept.push_str(rest[..start].trim_end_matches([' ', '\t']));
            removed += 1;
            rest = rest[end..].strip_prefix('\n').unwrap_or(&rest[end..]);
        } else {
            kept.push_str(&rest[..end]);
            rest = &rest[end..];
        }
    }
    kept.push_str(rest);

    if removed == 0 {
        return 0;
    }
    let backup = path.with_extension("xml.extraspace-bak");
    if !backup.exists() {
        let _ = std::fs::copy(&path, &backup);
    }
    match std::fs::write(&path, kept) {
        Ok(()) => {
            warn!(
                removed,
                path = %path.display(),
                "removed saved virtual-monitor layouts; they crash mutter 50.4 when the mode no longer exists"
            );
            removed
        }
        Err(e) => {
            warn!(error = %e, "could not rewrite monitors.xml");
            0
        }
    }
}

fn dirs_config_dir() -> Option<std::path::PathBuf> {
    if let Ok(dir) = std::env::var("XDG_CONFIG_HOME") {
        if !dir.is_empty() {
            return Some(std::path::PathBuf::from(dir));
        }
    }
    std::env::var("HOME")
        .ok()
        .map(|h| std::path::PathBuf::from(h).join(".config"))
}

pub async fn list_connectors_best_effort(conn: &Connection) -> Vec<String> {
    match list_connectors(conn).await {
        Ok(v) => v,
        Err(e) => {
            warn!(
                error = %e,
                "could not parse DisplayConfig.GetCurrentState; mirror sources unavailable"
            );
            Vec::new()
        }
    }
}
