//! Holds a screen-cast node open so an external client can inspect what mutter
//! offers for buffer negotiation.
//!
//! Mirrors an existing connector rather than creating a virtual monitor, so it
//! answers "does this mutter hand out dma-bufs, and in which formats" without
//! rearranging the desktop.
//!
//! ```console
//! cargo run -p xs-mutter --example dmabuf_probe -- 60
//! # then, against the printed node id:
//! gst-launch-1.0 -v pipewiresrc path=NODE ! 'video/x-raw(memory:DMABuf)' ! fakesink
//! ```

use std::time::Duration;

use xs_mutter::{CaptureSource, CursorMode, DisplayConfig};

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,xs_mutter=debug".into()),
        )
        .init();

    let hold: u64 = std::env::args()
        .nth(1)
        .and_then(|a| a.parse().ok())
        .unwrap_or(60);

    let connectors = xs_mutter::list_monitors().await?;
    let connector = connectors
        .first()
        .cloned()
        .ok_or_else(|| anyhow::anyhow!("mutter reported no connectors to mirror"))?;
    println!("mirroring {connector} (of {connectors:?})");

    let session = xs_mutter::Session::open(DisplayConfig {
        width: 0,
        height: 0,
        refresh_rate: 60.0,
        scale: 1.0,
        cursor_mode: CursorMode::Metadata,
        source: CaptureSource::Monitor(connector),
        fallback_sizes: Vec::new(),
    })
    .await?;

    println!("\n  pipewire node id : {}", session.node_id());
    println!("\nholding for {hold}s");
    tokio::time::sleep(Duration::from_secs(hold)).await;
    session.close().await?;
    Ok(())
}
