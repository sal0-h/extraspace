//! Read mutter's current monitor list, and turn the virtual monitor on.
//!
//! `ApplyMonitorsConfig` used to race PipeWire format negotiation on stock
//! mutter 50.4 and SIGSEGV gnome-shell. The local libmutter rebuild guards those
//! NULL paths, so after the PipeWire node exists we can ask mutter to put the
//! new Meta-0 into the layout. A new virtual serial can also make mutter pick a
//! fresh layout that resets existing monitor transforms. Preserve the layout
//! from before RecordVirtual in either case.

use std::collections::HashMap;
use std::time::Duration;

use tracing::{info, warn};
use zbus::Connection;
use zvariant::{OwnedValue, Value};

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
type AppliedLogicalMonitor = (
    i32,
    i32,
    f64,
    u32,
    bool,
    Vec<(String, String, HashMap<String, Value<'static>>)>,
);

const APPLY_TEMPORARY: u32 = 1;
const LAYOUT_LOGICAL: u32 = 1;
const VIRTUAL_WAIT: Duration = Duration::from_millis(50);
const VIRTUAL_WAIT_TRIES: u32 = 100;

/// The monitor configuration before RecordVirtual changes the monitor set.
/// Mutter may choose a fresh layout as soon as the virtual output appears, so
/// its state at that point is not a reliable source for the existing displays.
pub(crate) struct DisplayLayout {
    monitors: Vec<Monitor>,
    logical: Vec<LogicalMonitor>,
    properties: HashMap<String, OwnedValue>,
}

/// The physical layout as it stood at disconnect, plus the output to wait for.
pub(crate) struct LayoutAfterRemoval {
    removed_spec: MonitorSpec,
    layout: DisplayLayout,
}

pub(crate) async fn capture_display_layout(conn: &Connection) -> Result<DisplayLayout> {
    let (_serial, monitors, logical, properties) = get_current_state(conn).await?;
    Ok(DisplayLayout {
        monitors,
        logical,
        properties,
    })
}

/// Keep the user's latest physical layout, including changes made while the
/// tablet was connected. Remove only the virtual output owned by this session.
pub(crate) async fn capture_layout_for_virtual_removal(
    conn: &Connection,
    before: &DisplayLayout,
) -> Result<Option<LayoutAfterRemoval>> {
    let layout = capture_display_layout(conn).await?;
    let Some((spec, ..)) = new_virtual_monitor(&layout.monitors, before) else {
        return Ok(None);
    };
    let removed_spec = spec.clone();
    let layout = layout_without_monitor(layout, &removed_spec)?;
    Ok(Some(LayoutAfterRemoval {
        removed_spec,
        layout,
    }))
}

fn layout_without_monitor(
    mut layout: DisplayLayout,
    removed_spec: &MonitorSpec,
) -> Result<DisplayLayout> {
    layout.monitors.retain(|(spec, ..)| *spec != *removed_spec);
    for (_, _, _, _, _, specs, _) in &mut layout.logical {
        specs.retain(|spec| *spec != *removed_spec);
    }
    layout.logical.retain(|(.., specs, _)| !specs.is_empty());
    if layout.logical.is_empty() {
        return Err(Error::DisplayLayout(
            "no physical monitor remains after removing the virtual display".into(),
        ));
    }

    // Mutter requires the remaining layout to start at the origin. The virtual
    // display may have been placed to the left or above a physical display.
    let min_x = layout
        .logical
        .iter()
        .map(|logical| logical.0)
        .min()
        .unwrap();
    let min_y = layout
        .logical
        .iter()
        .map(|logical| logical.1)
        .min()
        .unwrap();
    for (x, y, ..) in &mut layout.logical {
        *x -= min_x;
        *y -= min_y;
    }
    if !layout.logical.iter().any(|logical| logical.4) {
        layout.logical[0].4 = true;
    }

    Ok(layout)
}

/// Mutter reloads saved display settings when Meta-0 disappears. Restore the
/// physical layout that was in use at disconnect once that reload has finished.
pub(crate) async fn restore_layout_after_virtual_removal(
    conn: &Connection,
    expected: &LayoutAfterRemoval,
) -> Result<()> {
    let mut state = None;
    for _ in 0..VIRTUAL_WAIT_TRIES {
        let candidate = get_current_state(conn).await?;
        if !candidate
            .1
            .iter()
            .any(|(spec, ..)| spec == &expected.removed_spec)
        {
            state = Some(candidate);
            break;
        }
        tokio::time::sleep(VIRTUAL_WAIT).await;
    }
    let Some((serial, monitors, logical, properties)) = state else {
        return Err(Error::DisplayLayout(
            "the virtual monitor did not disappear from DisplayConfig".into(),
        ));
    };

    if monitors.len() != expected.layout.monitors.len()
        || !expected
            .layout
            .monitors
            .iter()
            .all(|(old_spec, ..)| monitors.iter().any(|(now_spec, ..)| now_spec == old_spec))
    {
        return Err(Error::DisplayLayout(
            "the connected monitors changed while stopping the stream".into(),
        ));
    }
    if layout_matches(&expected.layout, &monitors, &logical, &properties, None) {
        return Ok(());
    }

    let layout_mode =
        prop_u32(&expected.layout.properties, "layout-mode").unwrap_or(LAYOUT_LOGICAL);
    let applied = build_applied_existing_layout(&expected.layout, &monitors)?;
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
    info!("restored the physical monitor layout after disconnect");
    Ok(())
}

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

/// Add the virtual monitor to the layout that existed before RecordVirtual.
///
/// Mutter can regenerate the whole configuration when a new output appears,
/// including the transforms of existing monitors. Reading the layout only after
/// that reload would preserve the wrong orientation. This runs only with the
/// patched mutter: ApplyMonitorsConfig races screen-cast setup on stock 50.4.
pub(crate) async fn enable_virtual_monitor(
    conn: &Connection,
    wanted_scale: f64,
    before: &DisplayLayout,
) -> Result<()> {
    let mut state = None;
    for _ in 0..VIRTUAL_WAIT_TRIES {
        let candidate = get_current_state(conn).await?;
        if new_virtual_monitor(&candidate.1, before).is_some() {
            state = Some(candidate);
            break;
        }
        tokio::time::sleep(VIRTUAL_WAIT).await;
    }
    let Some((serial, monitors, logical, properties)) = state else {
        return Err(Error::DisplayLayout(
            "the new virtual monitor did not appear in DisplayConfig".into(),
        ));
    };

    let (spec, modes, mon_props) = new_virtual_monitor(&monitors, before)
        .ok_or_else(|| Error::DisplayLayout("the new virtual monitor disappeared".into()))?;
    if monitors.len() != before.monitors.len() + 1
        || !before
            .monitors
            .iter()
            .all(|(old_spec, ..)| monitors.iter().any(|(now_spec, ..)| now_spec == old_spec))
    {
        return Err(Error::DisplayLayout(
            "the connected monitors changed while starting the stream".into(),
        ));
    }

    let virtual_active = logical
        .iter()
        .any(|(.., specs, _)| specs.iter().any(|s| s == spec));
    let existing_layout_unchanged =
        layout_matches(before, &monitors, &logical, &properties, Some(spec));
    if virtual_active && existing_layout_unchanged {
        return Ok(());
    }
    if !existing_layout_unchanged {
        warn!("mutter changed the existing monitor layout while adding the virtual monitor; restoring it");
    }

    let preferred = modes
        .iter()
        .find(|mode| mode_flag(mode, "is-preferred"))
        .or_else(|| modes.first())
        .ok_or_else(|| Error::DisplayLayout("the virtual monitor has no modes".into()))?;

    let virtual_mode = preferred.0.clone();
    let virtual_scale = closest_scale(preferred, wanted_scale);
    let (virt_w, virt_h) = (preferred.1, preferred.2);

    let layout_mode = prop_u32(&before.properties, "layout-mode").unwrap_or(LAYOUT_LOGICAL);
    let (applied, x, y) = build_applied_layout(
        before,
        &monitors,
        spec,
        mon_props,
        &virtual_mode,
        virtual_scale,
        layout_mode,
    )?;

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

fn build_applied_layout(
    before: &DisplayLayout,
    monitors: &[Monitor],
    virtual_spec: &MonitorSpec,
    virtual_props: &HashMap<String, OwnedValue>,
    virtual_mode: &str,
    virtual_scale: f64,
    layout_mode: u32,
) -> Result<(Vec<AppliedLogicalMonitor>, i32, i32)> {
    let (x, y) = place_after_layout(&before.monitors, &before.logical, layout_mode);
    let mut applied = build_applied_existing_layout(before, monitors)?;

    applied.push((
        x,
        y,
        virtual_scale,
        0,
        !before.logical.iter().any(|(.., primary, _, _)| *primary),
        vec![(
            virtual_spec.0.clone(),
            virtual_mode.to_owned(),
            monitor_apply_props(virtual_props),
        )],
    ));
    Ok((applied, x, y))
}

fn build_applied_existing_layout(
    before: &DisplayLayout,
    monitors: &[Monitor],
) -> Result<Vec<AppliedLogicalMonitor>> {
    let mut applied = Vec::with_capacity(before.logical.len());

    for (lx, ly, scale, transform, primary, specs, _props) in &before.logical {
        let mut members = Vec::new();
        for spec in specs {
            let old_monitor = before
                .monitors
                .iter()
                .find(|(s, ..)| s == spec)
                .ok_or_else(|| {
                    Error::DisplayLayout(format!("{} vanished from the saved layout", spec.0))
                })?;
            let mode_id = current_mode_id(old_monitor)
                .ok_or_else(|| Error::DisplayLayout(format!("{} had no active mode", spec.0)))?;
            let now_monitor = monitors
                .iter()
                .find(|(s, ..)| s == spec)
                .ok_or_else(|| Error::DisplayLayout(format!("{} disconnected", spec.0)))?;
            if !now_monitor.1.iter().any(|mode| mode.0 == mode_id) {
                return Err(Error::DisplayLayout(format!(
                    "{} no longer offers mode {mode_id}",
                    spec.0
                )));
            }
            members.push((
                spec.0.clone(),
                mode_id.to_owned(),
                monitor_apply_props(&old_monitor.2),
            ));
        }
        applied.push((*lx, *ly, *scale, *transform, *primary, members));
    }

    Ok(applied)
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

fn new_virtual_monitor<'a>(monitors: &'a [Monitor], before: &DisplayLayout) -> Option<&'a Monitor> {
    monitors.iter().find(|(spec, ..)| {
        is_virtual(spec)
            && !before
                .monitors
                .iter()
                .any(|(old_spec, ..)| old_spec == spec)
    })
}

fn current_mode_id(monitor: &Monitor) -> Option<&str> {
    monitor
        .1
        .iter()
        .find(|mode| mode_flag(mode, "is-current"))
        .map(|mode| mode.0.as_str())
}

fn same_specs(a: &[MonitorSpec], b: &[MonitorSpec]) -> bool {
    a.len() == b.len() && a.iter().all(|spec| b.contains(spec))
}

fn layout_matches(
    before: &DisplayLayout,
    monitors: &[Monitor],
    logical: &[LogicalMonitor],
    properties: &HashMap<String, OwnedValue>,
    new_spec: Option<&MonitorSpec>,
) -> bool {
    if prop_u32(&before.properties, "layout-mode") != prop_u32(properties, "layout-mode") {
        return false;
    }

    let existing_logical: Vec<_> = logical
        .iter()
        .filter(|(.., specs, _)| new_spec.is_none_or(|spec| !specs.contains(spec)))
        .collect();
    if existing_logical.len() != before.logical.len() {
        return false;
    }

    before.logical.iter().all(|old| {
        existing_logical.iter().any(|now| {
            old.0 == now.0
                && old.1 == now.1
                && old.2 == now.2
                && old.3 == now.3
                && old.4 == now.4
                && same_specs(&old.5, &now.5)
        })
    }) && before.monitors.iter().all(|old| {
        monitors.iter().any(|now| {
            old.0 == now.0
                && current_mode_id(old) == current_mode_id(now)
                && prop_u32(&old.2, "color-mode") == prop_u32(&now.2, "color-mode")
                && prop_u32(&old.2, "rgb-range") == prop_u32(&now.2, "rgb-range")
                && prop_bool(&old.2, "is-underscanning") == prop_bool(&now.2, "is-underscanning")
        })
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
    if let Some(v) = props
        .get("is-underscanning")
        .and_then(|v| bool::try_from(v.clone()).ok())
    {
        out.insert("underscanning".into(), Value::from(v));
    }
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

fn place_after_layout(
    monitors: &[Monitor],
    logical: &[LogicalMonitor],
    layout_mode: u32,
) -> (i32, i32) {
    logical
        .iter()
        .filter_map(|(x, y, scale, transform, _, specs, _)| {
            let connector = specs.first()?.0.as_str();
            let (mw, mh) = current_mode_size(monitors, connector)?;
            let (lw, _) = logical_size(mw, mh, *scale, *transform, layout_mode);
            Some((x + lw, *y))
        })
        .max_by_key(|(right, _)| *right)
        .unwrap_or((0, 0))
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

    fn monitor_spec(connector: &str, vendor: &str) -> MonitorSpec {
        (
            connector.into(),
            vendor.into(),
            "model".into(),
            "serial".into(),
        )
    }

    fn active_monitor(spec: MonitorSpec, mode_id: &str, width: i32, height: i32) -> Monitor {
        let mode_props = HashMap::from([
            ("is-current".into(), OwnedValue::from(true)),
            ("is-preferred".into(), OwnedValue::from(true)),
        ]);
        (
            spec,
            vec![(
                mode_id.into(),
                width,
                height,
                60.0,
                1.0,
                vec![1.0],
                mode_props,
            )],
            HashMap::new(),
        )
    }

    fn logical_monitor(x: i32, transform: u32, primary: bool, spec: MonitorSpec) -> LogicalMonitor {
        (x, 0, 1.0, transform, primary, vec![spec], HashMap::new())
    }

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

    #[test]
    fn new_virtual_monitor_keeps_preexisting_monitor_layout() {
        let laptop = monitor_spec("eDP-1", "CMN");
        let external = monitor_spec("DP-1", "DEL");
        let virtual_spec = monitor_spec("Meta-0", "MetaVendor");
        let before = DisplayLayout {
            monitors: vec![
                active_monitor(laptop.clone(), "1920x1080", 1920, 1080),
                active_monitor(external.clone(), "1920x1080", 1920, 1080),
            ],
            logical: vec![
                logical_monitor(0, 2, true, laptop.clone()),
                logical_monitor(1920, 1, false, external.clone()),
            ],
            properties: HashMap::from([("layout-mode".into(), OwnedValue::from(LAYOUT_LOGICAL))]),
        };

        // Mutter's fallback configuration can enable Meta-0 while resetting
        // both physical transforms to normal. An already enabled virtual output
        // must not make us accept that changed layout.
        let monitors = vec![
            active_monitor(laptop.clone(), "1920x1080", 1920, 1080),
            active_monitor(external.clone(), "1920x1080", 1920, 1080),
            active_monitor(virtual_spec.clone(), "1200x1920", 1200, 1920),
        ];
        let logical = vec![
            logical_monitor(0, 0, true, laptop),
            logical_monitor(1920, 0, false, external),
            logical_monitor(3840, 0, false, virtual_spec.clone()),
        ];
        let properties = HashMap::from([("layout-mode".into(), OwnedValue::from(LAYOUT_LOGICAL))]);
        assert!(!layout_matches(
            &before,
            &monitors,
            &logical,
            &properties,
            Some(&virtual_spec),
        ));

        let (applied, x, y) = build_applied_layout(
            &before,
            &monitors,
            &virtual_spec,
            &HashMap::new(),
            "1200x1920",
            1.0,
            LAYOUT_LOGICAL,
        )
        .unwrap();
        assert_eq!((x, y), (3000, 0));
        assert_eq!(applied.len(), 3);
        assert_eq!(applied[0].3, 2);
        assert_eq!(applied[1].3, 1);
        assert_eq!(applied[0].5[0].1, "1920x1080");
        assert_eq!(applied[2].5[0].0, "Meta-0");
    }

    #[test]
    fn removing_virtual_keeps_the_latest_physical_orientation() {
        let physical = monitor_spec("eDP-1", "CMN");
        let virtual_spec = monitor_spec("Meta-0", "MetaVendor");
        let current = DisplayLayout {
            monitors: vec![
                active_monitor(physical.clone(), "1920x1080", 1920, 1080),
                active_monitor(virtual_spec.clone(), "2304x1440", 2304, 1440),
            ],
            logical: vec![
                logical_monitor(0, 0, true, virtual_spec.clone()),
                logical_monitor(1536, 2, false, physical.clone()),
            ],
            properties: HashMap::from([("layout-mode".into(), OwnedValue::from(LAYOUT_LOGICAL))]),
        };

        let remaining = layout_without_monitor(current, &virtual_spec).unwrap();
        assert_eq!(remaining.monitors.len(), 1);
        assert_eq!(remaining.logical.len(), 1);
        assert_eq!(remaining.logical[0].0, 0);
        assert_eq!(remaining.logical[0].3, 2);
        assert!(remaining.logical[0].4);
        assert_eq!(remaining.logical[0].5, vec![physical]);
    }
}
