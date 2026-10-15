//! The engine: everything that happens between "user flicks the switch" and
//! "desktop appears on the tablet".
//!
//! The GTK front end never touches a socket, a pipeline or a D-Bus session. It
//! sends [`Command`]s and renders [`Event`]s. That keeps all the blocking and
//! async work off the main loop, and means the whole engine can be driven from a
//! test or a future CLI without a window.

use std::sync::Arc;
use std::time::{Duration, Instant};

use tokio::sync::{broadcast, mpsc, watch};
use tracing::{debug, error, warn};

pub mod adaptive;
mod diagnostics;
mod session;

pub use adaptive::{AdaptiveController, BitrateBounds, HealthSample};
pub use session::{
    clamp_ui_scale, fallback_sizes, logical_size_for, modes_enabled, scaled_size_for,
    virtual_size_for, DisplayMode, SessionConfig,
};
pub use xs_transport::wireless::{PairedDevice, PairingOffer, Wireless, WirelessEvent};

/// Events per second the tablet reports and the controller consumes.
pub const STATS_HZ: u64 = 2;

/// What the UI asks the engine to do.
#[derive(Debug, Clone)]
pub enum Command {
    /// Look for a tablet and start streaming.
    Connect,
    /// Tear everything down.
    Disconnect,
    /// Rebuild the display at a new scale, preserving the tablet and connection.
    SetScale(f64),
    /// Switch between extending and mirroring.
    SetMode(DisplayMode),
    /// Change the quality envelope the adaptive controller works within.
    SetBitrateBounds(BitrateBounds),
    /// Start or stop camera passthrough.
    SetCamera {
        enabled: bool,
        camera_id: String,
    },
    /// Save a decodable video frame and negotiated pipeline caps in a new directory.
    SaveVideoDiagnostic(std::path::PathBuf),
    ForgetDevice(String),
    /// Shut the engine thread down.
    Shutdown,
}

/// Where the engine currently is. Drives what the UI shows.
#[derive(Debug, Clone, PartialEq, serde::Serialize)]
pub enum State {
    /// Nothing running.
    Idle,
    /// No tablet plugged in, or adb cannot see one.
    NoTablet,
    /// Tablet present but the user has not accepted the USB-debugging prompt.
    /// Worth its own state because it is the single most common first-run failure
    /// and the fix is entirely in the user's hands.
    Unauthorized { device: String },
    /// Installing the companion app, forwarding ports, negotiating.
    Connecting { step: String },
    /// An established connection was interrupted; resume it when its app returns.
    WaitingForTablet { device: String, connection: String },
    /// Running.
    Streaming {
        device: String,
        connection: String,
        width: u32,
        height: u32,
        encoder: String,
    },
    /// Stopped because of an error. Carries something the user can act on.
    Failed { message: String },
}

/// Live numbers for the statistics panel.
#[derive(Debug, Clone, Default)]
pub struct Stats {
    pub bitrate_kbps: u32,
    pub fps: f64,
    pub latency_ms: f64,
    pub frames_encoded: u64,
    /// Cumulative frames released by the tablet decoder.
    pub frames_decoded: u64,
    pub frames_dropped: u64,
    pub decode_queue_depth: u32,
}

#[derive(Debug, Clone)]
pub enum Event {
    State(State),
    Stats(Stats),
    /// Non-fatal; shown as a toast rather than replacing the whole view.
    Warning(String),
    Wireless(WirelessEvent),
}

/// Handle the UI holds. Cloneable and cheap.
#[derive(Clone)]
pub struct EngineHandle {
    commands: mpsc::UnboundedSender<Command>,
    events: broadcast::Sender<Event>,
    finished: watch::Receiver<bool>,
    pub wireless: Option<Arc<Wireless>>,
}

impl EngineHandle {
    pub fn send(&self, command: Command) {
        if self.commands.send(command).is_err() {
            warn!("engine is gone; command dropped");
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<Event> {
        self.events.subscribe()
    }

    /// Request shutdown and wait until the display session has been closed.
    pub async fn shutdown(&self) {
        let mut finished = self.finished.clone();
        self.send(Command::Shutdown);
        while !*finished.borrow_and_update() {
            if finished.changed().await.is_err() {
                break;
            }
        }
    }
}

/// Starts the engine on its own runtime in a background thread.
///
/// A dedicated thread rather than sharing the GTK main loop: GStreamer and zbus
/// both want to block, and a stalled encoder must never freeze the window.
pub fn spawn(config: SessionConfig) -> EngineHandle {
    let wireless = if cfg!(test) {
        None
    } else {
        match Wireless::new() {
            Ok(wireless) => Some(wireless),
            Err(e) => {
                warn!(error = %e, "Wi-Fi unavailable");
                None
            }
        }
    };
    let (cmd_tx, cmd_rx) = mpsc::unbounded_channel();
    let (event_tx, _) = broadcast::channel(64);
    let (finished_tx, finished) = watch::channel(false);
    let handle = EngineHandle {
        commands: cmd_tx,
        events: event_tx.clone(),
        finished,
        wireless: wireless.clone(),
    };

    std::thread::Builder::new()
        .name("xs-engine".into())
        .spawn(move || {
            let runtime = match tokio::runtime::Builder::new_multi_thread()
                .worker_threads(2)
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(e) => {
                    error!(error = %e, "could not start engine runtime");
                    let _ = event_tx.send(Event::State(State::Failed {
                        message: format!("could not start engine: {e}"),
                    }));
                    let _ = finished_tx.send(true);
                    return;
                }
            };
            runtime.block_on(async {
                let service = wireless.as_ref().map(|wireless| {
                    let wireless = Arc::clone(wireless);
                    let events = event_tx.clone();
                    tokio::spawn(async move {
                        if let Err(e) = wireless.serve().await {
                            warn!(error = %e, "Wi-Fi service stopped");
                            let _ = events.send(Event::Warning(format!("Wi-Fi: {e}")));
                        }
                    })
                });
                let updates = wireless.as_ref().map(|wireless| {
                    let mut updates = wireless.subscribe();
                    let events = event_tx.clone();
                    tokio::spawn(async move {
                        while let Ok(update) = updates.recv().await {
                            let _ = events.send(Event::Wireless(update));
                        }
                    })
                });
                session::run(config, cmd_rx, event_tx, wireless).await;
                if let Some(task) = service {
                    task.abort();
                }
                if let Some(task) = updates {
                    task.abort();
                }
            });
            let _ = finished_tx.send(true);
            debug!("engine thread finished");
        })
        .expect("spawning the engine thread should not fail");

    handle
}

/// Shared helper so both the engine and the UI format rates the same way.
pub fn format_bitrate(kbps: u32) -> String {
    if kbps >= 1000 {
        format!("{:.1} Mbps", kbps as f64 / 1000.0)
    } else {
        format!("{kbps} kbps")
    }
}

/// Tracks frames-per-second over a short sliding window.
pub(crate) struct FpsCounter {
    window: Duration,
    samples: Vec<(Instant, u64)>,
}

impl FpsCounter {
    pub fn new() -> Self {
        Self {
            window: Duration::from_secs(2),
            samples: Vec::with_capacity(8),
        }
    }

    pub fn sample(&mut self, now: Instant, frames: u64) {
        if self.samples.last().is_some_and(|&(_, last)| frames < last) {
            self.samples.clear();
        }
        self.samples.push((now, frames));
        let cutoff = now - self.window;
        self.samples.retain(|&(time, _)| time >= cutoff);
    }

    pub fn fps(&self) -> f64 {
        if self.samples.len() < 2 {
            return 0.0;
        }
        let &(first_time, first_frames) = self.samples.first().unwrap();
        let &(last_time, last_frames) = self.samples.last().unwrap();
        let span = last_time.duration_since(first_time);
        if span.is_zero() {
            return 0.0;
        }
        (last_frames - first_frames) as f64 / span.as_secs_f64()
    }
}

/// Shared counters the engine updates and the UI reads.
#[derive(Debug, Default)]
pub struct SharedStats {
    pub inner: std::sync::Mutex<Stats>,
}

impl SharedStats {
    pub fn get(&self) -> Stats {
        self.inner.lock().map(|s| s.clone()).unwrap_or_default()
    }
}

pub type SharedStatsRef = Arc<SharedStats>;

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn shutdown_waits_for_the_engine_thread() {
        let engine = spawn(SessionConfig::default());
        tokio::time::timeout(Duration::from_secs(2), engine.shutdown())
            .await
            .expect("engine shutdown timed out");
        assert!(*engine.finished.borrow());
    }

    #[test]
    fn formats_bitrate_readably() {
        assert_eq!(format_bitrate(15_000), "15.0 Mbps");
        assert_eq!(format_bitrate(6_500), "6.5 Mbps");
        assert_eq!(format_bitrate(800), "800 kbps");
    }

    #[test]
    fn fps_counter_measures_a_steady_stream() {
        let mut c = FpsCounter::new();
        let t0 = Instant::now();
        // Four health reports per second, each accounting for 15 video frames.
        for i in 0..9 {
            c.sample(t0 + Duration::from_millis(250 * i), 15 * i);
        }
        assert!((c.fps() - 60.0).abs() < 1.0, "got {}", c.fps());
    }

    #[test]
    fn fps_counter_is_zero_before_it_has_evidence() {
        let mut c = FpsCounter::new();
        assert_eq!(c.fps(), 0.0);
        c.sample(Instant::now(), 20);
        assert_eq!(c.fps(), 0.0);
    }

    #[test]
    fn fps_counter_forgets_old_ticks() {
        let mut c = FpsCounter::new();
        let t0 = Instant::now();
        for i in 0..60 {
            c.sample(t0 + Duration::from_micros(16_667 * i), i);
        }
        // Ten seconds later, a single tick: the old window must have aged out.
        c.sample(t0 + Duration::from_secs(10), 60);
        assert_eq!(c.fps(), 0.0);
    }

    #[test]
    fn health_reports_without_new_frames_do_not_count_as_video() {
        let mut c = FpsCounter::new();
        let t0 = Instant::now();
        c.sample(t0, 100);
        c.sample(t0 + Duration::from_millis(500), 100);
        assert_eq!(c.fps(), 0.0);
    }

    #[test]
    fn fps_counter_restarts_when_the_decoder_counter_resets() {
        let mut c = FpsCounter::new();
        let t0 = Instant::now();
        c.sample(t0, 100);
        c.sample(t0 + Duration::from_millis(500), 130);
        c.sample(t0 + Duration::from_secs(1), 0);
        assert_eq!(c.fps(), 0.0);
        c.sample(t0 + Duration::from_millis(1500), 30);
        assert_eq!(c.fps(), 60.0);
    }
}
