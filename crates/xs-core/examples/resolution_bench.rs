//! Streams to the real tablet and emits measurements as JSON lines.
//! Run with the desktop app closed. XS_MUTTER_MODES selects the capture mode;
//! XS_BENCH_SECONDS bounds the run. Settings come from the desktop app's config.
//! Uses the installed companion app. A GTK workload can be run alongside it.

use std::io::{self, BufRead};
use std::path::PathBuf;
use std::time::{Duration, Instant};

use serde_json::json;
use xs_core::{BitrateBounds, Command, DisplayMode, Event, SessionConfig, State};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_writer(io::stderr)
        .with_ansi(false)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,xs_core=debug,xs_video=debug".into()),
        )
        .init();
    let home = PathBuf::from(std::env::var_os("HOME").unwrap_or_default());
    let config_path = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"))
        .join("extraspace/config.json");
    let settings: serde_json::Value = serde_json::from_slice(&std::fs::read(config_path)?)?;
    let duration: u64 = std::env::var("XS_BENCH_SECONDS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(60);
    let config = SessionConfig {
        mode: DisplayMode::Extend,
        scale: settings["scale"].as_f64().unwrap_or(1.5),
        framerate: settings["framerate"].as_u64().unwrap_or(60) as u32,
        bounds: BitrateBounds {
            min_kbps: settings["min_bitrate_kbps"].as_u64().unwrap_or(6000) as u32,
            max_kbps: settings["max_bitrate_kbps"].as_u64().unwrap_or(50000) as u32,
        },
        camera_enabled: settings["camera_enabled"].as_bool().unwrap_or(false),
        camera_id: settings["camera_id"].as_str().unwrap_or("0").to_owned(),
        ..SessionConfig::default()
    };
    let engine = xs_core::spawn(config);
    let mut events = engine.subscribe();
    let (requests_tx, mut requests_rx) = tokio::sync::mpsc::unbounded_channel();
    std::thread::spawn(move || {
        for line in io::stdin().lock().lines().map_while(Result::ok) {
            if requests_tx.send(line).is_err() {
                break;
            }
        }
    });
    engine.send(Command::Connect);
    let deadline = tokio::time::sleep(Duration::from_secs(duration));
    tokio::pin!(deadline);
    let started = Instant::now();
    let mut connected = false;
    loop {
        tokio::select! {
            _ = &mut deadline => break,
            request = requests_rx.recv() => {
                let Some(request) = request else { break };
                if request == "quit" { break; }
                if let Some(path) = request.strip_prefix("snapshot ") {
                    engine.send(Command::SaveVideoDiagnostic(PathBuf::from(path)));
                }
            }
            event = events.recv() => {
                match event? {
                    Event::State(State::Streaming { width, height, encoder, .. }) => {
                        connected = true;
                        println!("{}", json!({"event":"streaming", "width":width, "height":height, "encoder":encoder}));
                    }
                    Event::State(State::Failed { message }) => {
                        eprintln!("{message}");
                        break;
                    }
                    Event::Stats(stats) => {
                        println!("{}", json!({
                            "event":"stats", "elapsed_s":started.elapsed().as_secs_f64(),
                            "encoded":stats.frames_encoded, "decoded":stats.frames_decoded,
                            "dropped":stats.frames_dropped, "queue":stats.decode_queue_depth,
                            "kbps":stats.bitrate_kbps, "rtt_ms":stats.latency_ms, "fps":stats.fps,
                        }));
                    }
                    Event::Warning(message) => eprintln!("{message}"),
                    _ => {}
                }
            }
        }
    }
    engine.shutdown().await;
    anyhow::ensure!(connected, "no real tablet stream was established");
    Ok(())
}
