//! Deciding whether this mutter is safe to ask for a scaled mode.
//!
//! Stock mutter 50.4 dereferences an unassigned CRTC config when `RecordVirtual`
//! is given `modes`, which SIGSEGVs gnome-shell and logs the user out. The fix is
//! a local rebuild of libmutter with the NULL guards, so there is no version
//! number that tells us the crash is gone -- a patched 50.4-2 and a stock 50.4-2
//! would look identical.
//!
//! Instead the rebuild script records which pacman version it patched, and we
//! only pass `modes` while the installed mutter is still that exact build. A
//! system upgrade replaces libmutter with a stock one and the versions stop
//! matching, so the scaled path switches itself off before it can crash anyone.

use std::path::PathBuf;
use std::sync::OnceLock;

use tracing::{info, warn};

const PACMAN_LOCAL_DB: &str = "/var/lib/pacman/local";

/// Whether to ask mutter for a scaled mode, i.e. panel resolution with a GNOME
/// logical scale.
///
/// Requires `XS_MUTTER_MODES=1` *and* a mutter recorded as patched. Evaluated
/// once, because it reads the filesystem and logs why it said no.
pub fn scaled_modes_allowed() -> bool {
    static ALLOWED: OnceLock<bool> = OnceLock::new();
    *ALLOWED.get_or_init(|| {
        if std::env::var_os("XS_MUTTER_MODES").is_none_or(|v| v != "1") {
            return false;
        }
        match (patched_version(), installed_version()) {
            (Some(patched), Some(installed)) if patched == installed => {
                if running_shell_is_stale() {
                    warn!(
                        "libmutter has been replaced since gnome-shell started, so the running \
                         compositor is still the unpatched one. Log out and back in; \
                         XS_MUTTER_MODES is being ignored until then"
                    );
                    return false;
                }
                info!(mutter = %installed, "mutter is patched; using a scaled virtual monitor");
                true
            }
            (Some(patched), Some(installed)) => {
                warn!(
                    patched = %patched,
                    installed = %installed,
                    "mutter changed since it was patched, so XS_MUTTER_MODES is being ignored. \
                     Rebuild with ~/build/mutter-patched/build-and-install.sh"
                );
                false
            }
            (None, _) => {
                warn!(
                    "XS_MUTTER_MODES=1 but no patched mutter is recorded, so it is being ignored. \
                     Asking stock mutter for a scaled mode logs you out"
                );
                false
            }
            (_, None) => {
                warn!("cannot tell which mutter is installed, so XS_MUTTER_MODES is being ignored");
                false
            }
        }
    })
}

/// Whether gnome-shell is still running against a libmutter that has since been
/// replaced on disk.
///
/// Installing the patched package does not fix the compositor that is already
/// running: it keeps the old, unlinked library mapped until the session restarts.
/// Passing `modes` in that window crashes exactly as if nothing had been patched,
/// so the marker alone is not enough to say yes.
fn running_shell_is_stale() -> bool {
    let Some(pid) = gnome_shell_pid() else {
        return false;
    };
    let Ok(maps) = std::fs::read_to_string(format!("/proc/{pid}/maps")) else {
        return false;
    };
    maps.lines()
        .any(|line| line.contains("libmutter-") && line.trim_end().ends_with("(deleted)"))
}

fn gnome_shell_pid() -> Option<u32> {
    for entry in std::fs::read_dir("/proc").ok()?.flatten() {
        let name = entry.file_name();
        let Some(pid) = name.to_str().and_then(|n| n.parse::<u32>().ok()) else {
            continue;
        };
        let comm = std::fs::read_to_string(format!("/proc/{pid}/comm")).unwrap_or_default();
        if comm.trim() == "gnome-shell" {
            return Some(pid);
        }
    }
    None
}

/// The mutter version the local rebuild script last patched, e.g. `50.4-2`.
fn patched_version() -> Option<String> {
    let text = std::fs::read_to_string(marker_path()?).ok()?;
    let version = text.trim();
    (!version.is_empty()).then(|| version.to_owned())
}

fn marker_path() -> Option<PathBuf> {
    let base = match std::env::var_os("XDG_DATA_HOME") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => PathBuf::from(std::env::var_os("HOME")?).join(".local/share"),
    };
    Some(base.join("extraspace/patched-mutter"))
}

/// Installed mutter version, read straight from pacman's local database.
///
/// Reading the database avoids shelling out to `pacman -Q` on every connect. The
/// directory name already carries the version, but `mutter-devkit` and friends
/// share the prefix, so `%NAME%` is what decides.
fn installed_version() -> Option<String> {
    for entry in std::fs::read_dir(PACMAN_LOCAL_DB).ok()?.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if !name.starts_with("mutter-") {
            continue;
        }
        let Ok(desc) = std::fs::read_to_string(entry.path().join("desc")) else {
            continue;
        };
        if let Some(version) = version_of_mutter(&desc) {
            return Some(version);
        }
    }
    None
}

/// Pulls `%VERSION%` out of a pacman `desc`, but only for mutter itself.
fn version_of_mutter(desc: &str) -> Option<String> {
    let field = |key: &str| {
        desc.lines()
            .skip_while(|line| line.trim() != key)
            .nth(1)
            .map(str::trim)
    };
    if field("%NAME%")? != "mutter" {
        return None;
    }
    Some(field("%VERSION%")?.to_owned())
}

#[cfg(test)]
mod tests {
    use super::*;

    const MUTTER_DESC: &str = "%NAME%\nmutter\n\n%VERSION%\n50.4-2\n\n%DESC%\nWindow manager\n";

    #[test]
    fn reads_the_version_of_mutter_itself() {
        assert_eq!(version_of_mutter(MUTTER_DESC).as_deref(), Some("50.4-2"));
    }

    #[test]
    fn ignores_packages_that_merely_start_with_mutter() {
        let devkit = MUTTER_DESC.replace("mutter\n", "mutter-devkit\n");
        assert_eq!(version_of_mutter(&devkit), None);
    }

    #[test]
    fn a_stock_upgrade_stops_matching_the_patched_build() {
        // The whole point: same-looking version strings must compare unequal
        // once pacman has replaced the rebuilt package.
        assert_ne!(
            version_of_mutter(MUTTER_DESC),
            version_of_mutter(&MUTTER_DESC.replace("50.4-2", "50.5-1"))
        );
    }
}
