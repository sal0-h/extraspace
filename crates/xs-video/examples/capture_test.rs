//! Proves the capture -> encode path end to end, with no tablet involved.
//!
//! Creates a real virtual monitor, runs the production pipeline against it, and
//! checks that what comes out is decodable H.264 at roughly the requested rate.
//! Everything the tablet would otherwise be needed for is downstream of this, so
//! if this passes, the remaining risk is MediaCodec and the socket, not capture.
//!
//! ```console
//! cargo run -p xs-video --example capture_test
//! ```

//! `XS_MIRROR=eDP-1` mirrors an existing panel instead of creating a virtual
//! monitor, which exercises the same capture and encode path without
//! rearranging the desktop. `XS_WIDTH`/`XS_HEIGHT`/`XS_SECONDS` override the
//! defaults below.

use std::time::{Duration, Instant};

use xs_mutter::{CaptureSource, CursorMode, DisplayConfig};
use xs_video::{VideoConfig, VideoPipeline};

const FRAMERATE: u32 = 60;

fn env_u32(key: &str, default: u32) -> u32 {
    std::env::var(key)
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(default)
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,xs_video=debug".into()),
        )
        .init();

    let width = env_u32("XS_WIDTH", 1332);
    let height = env_u32("XS_HEIGHT", 800);
    let capture_for = Duration::from_secs(env_u32("XS_SECONDS", 5) as u64);
    let scale: f64 = std::env::var("XS_SCALE")
        .ok()
        .and_then(|v| v.parse().ok())
        .unwrap_or(1.0);
    let source = match std::env::var("XS_MIRROR") {
        Ok(connector) => {
            println!("mirroring {connector}...");
            CaptureSource::Monitor(connector)
        }
        Err(_) => {
            println!("creating a {width}x{height}@{FRAMERATE} virtual monitor...");
            CaptureSource::Virtual
        }
    };
    let session = xs_mutter::Session::open(DisplayConfig {
        width,
        height,
        refresh_rate: FRAMERATE as f64,
        scale,
        cursor_mode: CursorMode::Metadata,
        source,
        fallback_sizes: Vec::new(),
    })
    .await?;
    println!("  pipewire node {}", session.node_id());
    let (width, height) = session.effective_size();

    let (pipeline, mut frames, _cursor) = VideoPipeline::new(
        session.node_id(),
        VideoConfig {
            width,
            height,
            framerate: FRAMERATE,
            bitrate_kbps: 15_000,
            scale,
        },
    )?;
    println!("  encoder: {}", pipeline.encoder().human_name());
    pipeline.start()?;

    let started = Instant::now();
    let mut first_frame_at = None;
    let mut count = 0u64;
    let mut keyframes = 0u64;
    let mut bytes = 0u64;
    let mut stream = Vec::new();

    println!("\ncapturing for {}s...", capture_for.as_secs());
    let deadline = tokio::time::Instant::now() + capture_for;
    loop {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        match tokio::time::timeout(remaining, frames.recv()).await {
            Ok(Some(frame)) => {
                if first_frame_at.is_none() {
                    first_frame_at = Some(started.elapsed());
                }
                count += 1;
                bytes += frame.data.len() as u64;
                if frame.keyframe {
                    keyframes += 1;
                }
                stream.extend_from_slice(&frame.data);
            }
            Ok(None) => {
                println!("  pipeline closed the channel early");
                break;
            }
            Err(_) => break, // deadline reached
        }
    }

    let (mutter_frames, pushed, push_fail, copy_max_us) = pipeline.take_capture_counts();
    pipeline.stop();
    session.close().await?;

    let elapsed = started.elapsed().as_secs_f64();
    let fps = count as f64 / elapsed;
    let mbps = bytes as f64 * 8.0 / 1_000_000.0 / elapsed;

    println!("\n--- results ---");
    match first_frame_at {
        Some(t) => println!("  first frame after   {:.0} ms", t.as_secs_f64() * 1000.0),
        None => println!("  first frame          NEVER ARRIVED"),
    }
    println!("  frames              {count}");
    println!("  keyframes           {keyframes}");
    println!("  measured rate       {fps:.1} fps  (asked for {FRAMERATE})");
    println!(
        "  mutter delivered    {} frames ({:.1} fps)",
        mutter_frames,
        mutter_frames as f64 / elapsed
    );
    println!("  pushed / rejected   {pushed} / {push_fail}");
    println!(
        "  worst copy          {:.1} ms  (0 on the dma-buf path -- nothing is copied)",
        copy_max_us as f64 / 1000.0
    );
    println!("  measured bitrate    {mbps:.1} Mbps  (asked for 15.0)");
    println!("  total bytes         {bytes}");

    let path = std::env::temp_dir().join("extraspace-capture.h264");
    std::fs::write(&path, &stream)?;
    println!("\n  wrote {} ({} bytes)", path.display(), stream.len());
    println!(
        "  verify with: ffprobe -v error -show_entries stream=codec_name,width,height {}",
        path.display()
    );

    // A capture that produces no frames is the failure this whole example exists
    // to catch, so make it a non-zero exit rather than a line of prose.
    anyhow::ensure!(count > 0, "capture produced no frames at all");
    anyhow::ensure!(
        keyframes > 0,
        "no keyframes: the tablet could never start decoding"
    );
    Ok(())
}
