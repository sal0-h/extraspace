//! Read mutter's current monitor list, and turn the virtual monitor on.
//!
//! `ApplyMonitorsConfig` used to race PipeWire format negotiation on stock
//! mutter 50.4 and SIGSEGV gnome-shell. The local libmutter rebuild guards those
//! NULL paths, so after the PipeWire node exists we can ask mutter to put the
//! new Meta-0 into the layout. Without that, a virtual serial that does not
//! match `monitors.xml` is created disabled and the user has to enable it in
//! Settings → Displays.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;
use zvariant::{OwnedValue, Value};

use crate::patched::patched_mutter_is_running;
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
type CurrentState = (
    u32,
    Vec<Monitor>,
    Vec<LogicalMonitor>,
    HashMap<String, OwnedValue>,
);

const APPLY_TEMPORARY: u32 = 1;
const LAYOUT_LOGICAL: u32 = 1;
const VIRTUAL_WAIT: Duration = Duration::from_millis(50);
const VIRTUAL_WAIT_TRIES: u32 = 20;

/// Connector names mutter currently knows about (e.g. `["DP-3", "HDMI-1"]`).
pub async fn list_connectors(conn: &Connection) -> Result<Vec<String>> {
    let (_serial, monitors, _logical, _properties) = get_current_state(conn).await?;
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
    let (_serial, monitors, logical, _properties) = get_current_state(conn).await.ok()?;

    let (spec, modes, _props) = monitors.into_iter().find(|(spec, ..)| is_virtual(spec))?;
    let (_id, width, height, ..) = modes
        .into_iter()
        .find(|mode| mode_flag(mode, "is-current"))?;
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

/// If the virtual monitor exists but is not in the layout, turn it on to the
/// right of the primary display.
///
/// Skipped on stock mutter: applying a layout races the screen-cast source and
/// used to log the user out. After a successful apply, Settings → Displays
/// shows the tablet as an ordinary monitor.
pub async fn enable_virtual_monitor(conn: &Connection, wanted_scale: f64) -> Result<()> {
    if !patched_mutter_is_running() {
        return Ok(());
    }

    let mut state = None;
    for _ in 0..VIRTUAL_WAIT_TRIES {
        let candidate = get_current_state(conn).await?;
        if candidate.1.iter().any(|(spec, ..)| is_virtual(spec)) {
            state = Some(candidate);
            break;
        }
        tokio::time::sleep(VIRTUAL_WAIT).await;
    }
    let Some((serial, monitors, logical, properties)) = state else {
        warn!("virtual monitor never appeared in DisplayConfig; leaving layout alone");
        return Ok(());
    };

    let Some((spec, modes, mon_props)) = monitors.iter().find(|(spec, ..)| is_virtual(spec)) else {
        return Ok(());
    };
    if logical
        .iter()
        .any(|(.., specs, _)| specs.iter().any(|s| s.0 == spec.0))
    {
        return Ok(());
    }

    let Some(preferred) = modes
        .iter()
        .find(|mode| mode_flag(mode, "is-preferred"))
        .or_else(|| modes.first())
    else {
        warn!("virtual monitor has no modes; cannot enable it");
        return Ok(());
    };

    let virtual_mode = preferred.0.clone();
    let virtual_scale = closest_scale(preferred, wanted_scale);
    let (virt_w, virt_h) = (preferred.1, preferred.2);

    let layout_mode = prop_u32(&properties, "layout-mode").unwrap_or(LAYOUT_LOGICAL);
    let (x, y) = place_right_of_primary(&monitors, &logical, layout_mode);

    let mut applied: Vec<(
        i32,
        i32,
        f64,
        u32,
        bool,
        Vec<(String, String, HashMap<String, Value<'static>>)>,
    )> = Vec::new();

    for (lx, ly, scale, transform, primary, specs, _props) in &logical {
        let mut members = Vec::new();
        for spec in specs {
            let Some((_, modes, props)) = monitors.iter().find(|(s, ..)| s.0 == spec.0) else {
                continue;
            };
            let mode = modes
                .iter()
                .find(|m| mode_flag(m, "is-current"))
                .or_else(|| modes.iter().find(|m| mode_flag(m, "is-preferred")))
                .or_else(|| modes.first());
            let Some(mode) = mode else {
                continue;
            };
            members.push((spec.0.clone(), mode.0.clone(), monitor_apply_props(props)));
        }
        if members.is_empty() {
            continue;
        }
        applied.push((*lx, *ly, *scale, *transform, *primary, members));
    }

    applied.push((
        x,
        y,
        virtual_scale,
        0,
        false,
        vec![(
            spec.0.clone(),
            virtual_mode.clone(),
            monitor_apply_props(mon_props),
        )],
    ));

    let mut apply_props: HashMap<String, Value<'static>> = HashMap::new();
    if prop_bool(&properties, "supports-changing-layout-mode") {
        apply_props.insert("layout-mode".into(), Value::from(layout_mode));
    }

    conn.call_method(
        Some("org.gnome.Mutter.DisplayConfig"),
        "/org/gnome/Mutter/DisplayConfig",
        Some("org.gnome.Mutter.DisplayConfig"),
        "ApplyMonitorsConfig",
        &(serial, APPLY_TEMPORARY, applied, apply_props),
    )
    .await?;

    info!(
        connector = %spec.0,
        mode = %virtual_mode,
        scale = virtual_scale,
        x,
        y,
        width = virt_w,
        height = virt_h,
        "turned the virtual monitor on"
    );
    Ok(())
}

async fn get_current_state(conn: &Connection) -> Result<CurrentState> {
    let reply = conn
        .call_method(
            Some("org.gnome.Mutter.DisplayConfig"),
            "/org/gnome/Mutter/DisplayConfig",
            Some("org.gnome.Mutter.DisplayConfig"),
            "GetCurrentState",
            &(),
        )
        .await?;
    reply.body().deserialize().map_err(|e| {
        Error::DBus(zbus::Error::Failure(format!(
            "GetCurrentState deserialize: {e}"
        )))
    })
}

fn is_virtual(spec: &MonitorSpec) -> bool {
    spec.1 == "MetaVendor"
}

fn mode_flag(mode: &Mode, key: &str) -> bool {
    mode.6
        .get(key)
        .and_then(|v| bool::try_from(v.clone()).ok())
        .unwrap_or(false)
}

fn prop_u32(props: &HashMap<String, OwnedValue>, key: &str) -> Option<u32> {
    props.get(key).and_then(|v| u32::try_from(v.clone()).ok())
}

fn prop_bool(props: &HashMap<String, OwnedValue>, key: &str) -> bool {
    props
        .get(key)
        .and_then(|v| bool::try_from(v.clone()).ok())
        .unwrap_or(false)
}

fn monitor_apply_props(props: &HashMap<String, OwnedValue>) -> HashMap<String, Value<'static>> {
    let mut out = HashMap::new();
    if let Some(v) = prop_u32(props, "color-mode") {
        out.insert("color-mode".into(), Value::from(v));
    }
    if let Some(v) = prop_u32(props, "rgb-range") {
        out.insert("rgb-range".into(), Value::from(v));
    }
    out
}

fn closest_scale(mode: &Mode, wanted: f64) -> f64 {
    let supported = &mode.5;
    if supported.is_empty() {
        return mode.4;
    }
    supported
        .iter()
        .copied()
        .min_by(|a, b| {
            (a - wanted)
                .abs()
                .partial_cmp(&(b - wanted).abs())
                .unwrap_or(std::cmp::Ordering::Equal)
        })
        .unwrap_or(mode.4)
}

fn transform_swaps_axes(transform: u32) -> bool {
    // Mutter: 90 / 270 / flipped-90 / flipped-270.
    matches!(transform, 1 | 3 | 5 | 7)
}

fn logical_size(
    width: i32,
    height: i32,
    scale: f64,
    transform: u32,
    layout_mode: u32,
) -> (i32, i32) {
    let (w, h) = if transform_swaps_axes(transform) {
        (height, width)
    } else {
        (width, height)
    };
    if layout_mode == LAYOUT_LOGICAL {
        (
            (w as f64 / scale).round() as i32,
            (h as f64 / scale).round() as i32,
        )
    } else {
        (w, h)
    }
}

fn current_mode_size(monitors: &[Monitor], connector: &str) -> Option<(i32, i32)> {
    let (_, modes, _) = monitors.iter().find(|(spec, ..)| spec.0 == connector)?;
    let mode = modes
        .iter()
        .find(|m| mode_flag(m, "is-current"))
        .or_else(|| modes.iter().find(|m| mode_flag(m, "is-preferred")))?;
    Some((mode.1, mode.2))
}

fn place_right_of_primary(
    monitors: &[Monitor],
    logical: &[LogicalMonitor],
    layout_mode: u32,
) -> (i32, i32) {
    let primary = logical
        .iter()
        .find(|(.., primary, _, _)| *primary)
        .or_else(|| logical.first());
    let Some((x, y, scale, transform, _, specs, _)) = primary else {
        return (0, 0);
    };
    let connector = specs.first().map(|s| s.0.as_str()).unwrap_or("");
    let (mw, mh) = current_mode_size(monitors, connector).unwrap_or((0, 0));
    let (lw, _) = logical_size(mw, mh, *scale, *transform, layout_mode);
    (x + lw, *y)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ninety_degree_transform_swaps_logical_size() {
        assert_eq!(
            logical_size(1920, 1080, 1.0, 1, LAYOUT_LOGICAL),
            (1080, 1920)
        );
        assert_eq!(
            logical_size(1920, 1080, 1.25, 0, LAYOUT_LOGICAL),
            (1536, 864)
        );
    }

    #[test]
    fn closest_scale_picks_supported_neighbour() {
        let mode: Mode = (
            "2304x1440@60.000".into(),
            2304,
            1440,
            60.0,
            1.5,
            vec![1.0, 1.3333333730697632, 1.5, 2.0],
            HashMap::new(),
        );
        assert!((closest_scale(&mode, 1.5) - 1.5).abs() < f64::EPSILON);
        assert!((closest_scale(&mode, 1.4) - 1.3333333730697632).abs() < 0.01);
    }
}
