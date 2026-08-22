//! Sparse frame-pacing probes.
//!
//! Only gaps at or above [`GAP_WARN`] are logged individually. Running maxima
//! are stored so the session's 2 Hz health line can print a compact window
//! without a line per frame.

use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

use gst::prelude::*;
use gstreamer as gst;
use tracing::warn;

/// Visible hitch territory at 60 fps: a missed frame plus a little jitter.
pub const GAP_WARN: std::time::Duration = std::time::Duration::from_millis(50);

pub struct StagePace {
    name: &'static str,
    last_ns: AtomicU64,
    max_gap_us: AtomicU64,
    frames: AtomicU64,
    gaps: AtomicU64,
}

impl StagePace {
    pub fn new(name: &'static str) -> Self {
        Self {
            name,
            last_ns: AtomicU64::new(0),
            max_gap_us: AtomicU64::new(0),
            frames: AtomicU64::new(0),
            gaps: AtomicU64::new(0),
        }
    }

    pub fn observe(&self, bytes: usize, pts_us: u64, extra: Option<&'static str>) {
        let now = monotonic_ns();
        let last = self.last_ns.swap(now, Ordering::Relaxed);
        self.frames.fetch_add(1, Ordering::Relaxed);
        if last == 0 {
            return;
        }
        let dt_us = now.saturating_sub(last) / 1000;
        fetch_max(&self.max_gap_us, dt_us);
        if dt_us >= GAP_WARN.as_micros() as u64 {
            self.gaps.fetch_add(1, Ordering::Relaxed);
            warn!(
                stage = self.name,
                dt_ms = dt_us / 1000,
                bytes,
                pts_us,
                extra = extra.unwrap_or(""),
                "pacing gap"
            );
        }
    }

    /// `(frames, gaps, max_gap_us)` since the previous take.
    pub fn take(&self) -> (u64, u64, u64) {
        (
            self.frames.swap(0, Ordering::Relaxed),
            self.gaps.swap(0, Ordering::Relaxed),
            self.max_gap_us.swap(0, Ordering::Relaxed),
        )
    }
}

pub fn attach_buffer_probe(
    element: &gst::Element,
    pad_name: &str,
    pace: &std::sync::Arc<StagePace>,
) {
    let Some(pad) = element.static_pad(pad_name) else {
        warn!(
            element = element.name().as_str(),
            pad_name, "no pad to probe"
        );
        return;
    };
    let pace = std::sync::Arc::clone(pace);
    pad.add_probe(gst::PadProbeType::BUFFER, move |_pad, info| {
        if let Some(buffer) = info.buffer() {
            let pts_us = buffer.pts().map(|t| t.useconds()).unwrap_or(0);
            pace.observe(buffer.size(), pts_us, None);
        }
        gst::PadProbeReturn::Ok
    });
}

fn fetch_max(slot: &AtomicU64, value: u64) {
    let mut current = slot.load(Ordering::Relaxed);
    while value > current {
        match slot.compare_exchange_weak(current, value, Ordering::Relaxed, Ordering::Relaxed) {
            Ok(_) => break,
            Err(seen) => current = seen,
        }
    }
}

fn monotonic_ns() -> u64 {
    use std::sync::OnceLock;
    static ORIGIN: OnceLock<Instant> = OnceLock::new();
    ORIGIN.get_or_init(Instant::now).elapsed().as_nanos() as u64
}
