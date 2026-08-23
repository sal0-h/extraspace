//! Capture from a mutter screen-cast PipeWire node, encode to H.264.
//!
//! The pipeline is deliberately boring:
//!
//! ```text
//! rust PipeWire capture -> appsrc -> videorate(drop-only)
//!            -> videoconvert -> I420 -> <encoder> -> h264parse -> appsink
//!
//! Cursor metadata is forwarded on the control channel; it is not baked into
//! the H.264 stream. A cursor-only PipeWire buffer therefore does not push a
//! video frame.
//! ```
//!
//! Capture is a single PipeWire consumer. A second client on the same mutter
//! node (for example `pipewiresrc` plus a cursor listener) makes gst-plugin-pipewire
//! abort on unfixed caps.
//!
//! `videorate` is configured **drop-only**: it caps the stream at the configured
//! rate and must never manufacture frames. The default `videorate` duplicates
//! the last buffer to fill holes, which on a damage-driven mutter capture
//! (idle ~11 fps) builds seconds of fake "catch-up" latency. Mutter only emits
//! a frame when something on the monitor actually changes; that is correct --
//! an idle screen costs almost no bandwidth -- but it means frame-count-based
//! reasoning is unreliable: see the keyframe note below.
//!
//! Encoded frames leave through a bounded channel. If the transport cannot keep
//! up, frames are dropped here rather than allowed to accumulate -- for a live
//! display, a stale frame has no value.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use bytes::Bytes;
use gst::prelude::*;
use gstreamer as gst;
use gstreamer_app::{AppSink, AppSrc};
use tokio::sync::mpsc;
use tracing::{debug, error, info, warn};

mod cursor;
mod encoder;
mod pacing;
pub use encoder::Encoder;
pub use pacing::StagePace;

/// `(frames, gaps, max_gap_us)` for one pipeline stage.
pub type PaceWindow = (u64, u64, u64);

/// Encoded frames buffered before we start dropping. One on purpose: a stale
/// frame has no value, and a deeper queue is how a short stall becomes a hitch.
const FRAME_QUEUE_DEPTH: usize = 1;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("GStreamer init failed: {0}")]
    Init(#[from] gst::glib::Error),

    #[error(
        "no usable H.264 encoder found. Install gstreamer1-plugins-ugly (for x264enc) \
         or gstreamer1-plugin-openh264."
    )]
    NoEncoder,

    #[error("could not build the '{element}' element -- is its GStreamer plugin installed?")]
    ElementMissing { element: &'static str },

    #[error("GStreamer pipeline error: {0}")]
    Pipeline(String),

    #[error("PipeWire capture failed: {0}")]
    Capture(String),

    #[error("failed to link the pipeline: {0}")]
    Link(#[from] gst::glib::BoolError),
}

pub type Result<T> = std::result::Result<T, Error>;

#[derive(Debug, Clone)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            width: 1920,
            height: 1080,
            framerate: 60,
            bitrate_kbps: 15_000,
        }
    }
}

/// One encoded access unit.
#[derive(Debug, Clone)]
pub struct EncodedFrame {
    pub data: Bytes,
    pub pts_us: u64,
    pub keyframe: bool,
}

/// Counters for the UI's statistics panel and the adaptive controller.
#[derive(Debug, Default)]
pub struct VideoStats {
    pub frames_encoded: AtomicU64,
    pub frames_dropped: AtomicU64,
    pub bytes_encoded: AtomicU64,
    pub last_au_bytes: AtomicU64,
    pub last_keyframe_bytes: AtomicU64,
}

impl VideoStats {
    /// `(encoded, dropped, bytes)`
    pub fn snapshot(&self) -> (u64, u64, u64) {
        (
            self.frames_encoded.load(Ordering::Relaxed),
            self.frames_dropped.load(Ordering::Relaxed),
            self.bytes_encoded.load(Ordering::Relaxed),
        )
    }
}

pub struct VideoPipeline {
    pipeline: gst::Pipeline,
    encoder_element: gst::Element,
    encoder: Encoder,
    stats: Arc<VideoStats>,
    config: VideoConfig,
    capture_pace: Arc<StagePace>,
    rate_pace: Arc<StagePace>,
    encode_pace: Arc<StagePace>,
    node_id: u32,
    hub: Arc<cursor::CursorHub>,
    capture: Mutex<Option<cursor::Capture>>,
}

impl VideoPipeline {
    /// Builds the pipeline for a mutter PipeWire node. Call [`start`](Self::start)
    /// to begin producing frames.
    pub fn new(
        node_id: u32,
        config: VideoConfig,
    ) -> Result<(Self, mpsc::Receiver<EncodedFrame>, mpsc::Receiver<()>)> {
        gst::init()?;

        let encoder = Encoder::detect().ok_or(Error::NoEncoder)?;
        let stats = Arc::new(VideoStats::default());
        let capture_pace = Arc::new(StagePace::new("capture"));
        let rate_pace = Arc::new(StagePace::new("videorate"));
        let encode_pace = Arc::new(StagePace::new("encoded"));
        let pipeline = gst::Pipeline::with_name("extraspace-display");

        let make = |name: &'static str| -> Result<gst::Element> {
            gst::ElementFactory::make(name)
                .build()
                .map_err(|_| Error::ElementMissing { element: name })
        };
        let caps_filter = |caps: gst::Caps| -> Result<gst::Element> {
            gst::ElementFactory::make("capsfilter")
                .property("caps", caps)
                .build()
                .map_err(|_| Error::ElementMissing {
                    element: "capsfilter",
                })
        };

        let overlay_caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRx")
            .field("width", config.width as i32)
            .field("height", config.height as i32)
            .field("framerate", gst::Fraction::new(config.framerate as i32, 1))
            .build();
        let overlay_src = AppSrc::builder()
            .name("cursor-overlay")
            .format(gst::Format::Time)
            .is_live(true)
            .block(false)
            .caps(&overlay_caps)
            .build();
        overlay_src.set_property("max-buffers", 1u64);
        overlay_src.set_property_from_str("leaky-type", "downstream");

        let rate = make("videorate")?;
        rate.set_property("drop-only", true);
        rate.set_property("skip-to-first", true);

        let rate_caps = caps_filter(
            gst::Caps::builder("video/x-raw")
                .field("framerate", gst::Fraction::new(config.framerate as i32, 1))
                .build(),
        )?;

        let convert = make("videoconvert")?;
        let convert_caps = caps_filter(
            gst::Caps::builder("video/x-raw")
                .field("format", "I420")
                .build(),
        )?;

        let encoder_element = encoder.build(config.bitrate_kbps, config.framerate)?;

        let parse = gst::ElementFactory::make("h264parse")
            // Repeat SPS/PPS on every keyframe: the tablet may attach late or
            // reconnect, and without inline headers its decoder cannot start.
            .property("config-interval", -1i32)
            .build()
            .map_err(|_| Error::ElementMissing {
                element: "h264parse",
            })?;

        let parse_caps = caps_filter(
            gst::Caps::builder("video/x-h264")
                .field("stream-format", "byte-stream")
                .field("alignment", "au")
                .build(),
        )?;

        let appsink = AppSink::builder()
            .name("frames")
            .max_buffers(FRAME_QUEUE_DEPTH as u32)
            .drop(true)
            // Push frames out as fast as they are encoded; do not pace to the clock.
            .sync(false)
            .build();

        let encode_elements = [
            overlay_src.upcast_ref(),
            &rate,
            &rate_caps,
            &convert,
            &convert_caps,
            &encoder_element,
            &parse,
            &parse_caps,
            appsink.upcast_ref(),
        ];
        pipeline.add_many(encode_elements)?;
        gst::Element::link_many(encode_elements)?;

        let (cursor_tx, cursor_rx) = mpsc::channel(1);
        let hub = cursor::CursorHub::new(config.framerate, cursor_tx);
        hub.attach_appsrc(overlay_src.clone());

        pacing::attach_buffer_probe(overlay_src.upcast_ref(), "src", &capture_pace);
        pacing::attach_buffer_probe(&rate, "src", &rate_pace);

        let (tx, rx) = mpsc::channel(FRAME_QUEUE_DEPTH);
        let sink_stats = Arc::clone(&stats);
        let sink_pace = Arc::clone(&encode_pace);

        appsink.set_callbacks(
            gstreamer_app::AppSinkCallbacks::builder()
                .new_sample(move |sink| {
                    let sample = sink.pull_sample().map_err(|_| gst::FlowError::Eos)?;
                    let buffer = sample.buffer().ok_or(gst::FlowError::Error)?;
                    let map = buffer.map_readable().map_err(|_| gst::FlowError::Error)?;

                    let keyframe = !buffer.flags().contains(gst::BufferFlags::DELTA_UNIT);
                    let pts_us = buffer.pts().map(|t| t.useconds()).unwrap_or(0);
                    let au_bytes = map.len();
                    let frame = EncodedFrame {
                        data: Bytes::copy_from_slice(map.as_slice()),
                        pts_us,
                        keyframe,
                    };

                    sink_stats
                        .bytes_encoded
                        .fetch_add(au_bytes as u64, Ordering::Relaxed);
                    sink_stats
                        .last_au_bytes
                        .store(au_bytes as u64, Ordering::Relaxed);
                    if keyframe {
                        sink_stats
                            .last_keyframe_bytes
                            .store(au_bytes as u64, Ordering::Relaxed);
                    }
                    sink_pace.observe(au_bytes, pts_us, keyframe.then_some("keyframe"));
                    match tx.try_send(frame) {
                        Ok(()) => {
                            sink_stats.frames_encoded.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            // Transport is behind. Dropping now keeps latency bounded;
                            // the adaptive controller sees this and lowers bitrate.
                            sink_stats.frames_dropped.fetch_add(1, Ordering::Relaxed);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            return Err(gst::FlowError::Eos);
                        }
                    }
                    Ok(gst::FlowSuccess::Ok)
                })
                .build(),
        );

        info!(
            node_id,
            encoder = encoder.element_name(),
            width = config.width,
            height = config.height,
            framerate = config.framerate,
            bitrate_kbps = config.bitrate_kbps,
            "display pipeline built"
        );

        Ok((
            Self {
                pipeline,
                encoder_element,
                encoder,
                stats,
                config,
                capture_pace,
                rate_pace,
                encode_pace,
                node_id,
                hub,
                capture: Mutex::new(None),
            },
            rx,
            cursor_rx,
        ))
    }

    /// Newest cursor overlay message. Stale positions are overwritten in the hub
    /// so the tablet never has to drain a backlog of pointer motion.
    pub fn take_cursor_message(&self) -> Option<xs_proto::CursorMessage> {
        self.hub.take_cursor_message()
    }

    pub fn start(&self) -> Result<()> {
        self.watch_bus();
        self.pipeline
            .set_state(gst::State::Playing)
            .map_err(|e| Error::Pipeline(e.to_string()))?;
        let mut capture = self.capture.lock().expect("capture lock");
        if capture.is_none() {
            match cursor::Capture::start(
                self.node_id,
                self.config.width,
                self.config.height,
                Arc::clone(&self.hub),
            ) {
                Ok(started) => *capture = Some(started),
                Err(e) => {
                    drop(capture);
                    let _ = self.pipeline.set_state(gst::State::Null);
                    return Err(Error::Capture(e));
                }
            }
        }
        Ok(())
    }

    pub fn stop(&self) {
        if let Ok(mut capture) = self.capture.lock() {
            *capture = None;
        }
        if let Err(e) = self.pipeline.set_state(gst::State::Null) {
            warn!(error = %e, "pipeline did not stop cleanly");
        }
    }

    /// Adjusts bitrate on the running pipeline; the adaptive controller's lever.
    pub fn set_bitrate(&self, kbps: u32) {
        self.encoder.set_bitrate(&self.encoder_element, kbps);
        debug!(kbps, "bitrate updated");
    }

    /// Asks the encoder for an immediate keyframe, with headers. Used when the
    /// tablet reconnects or reports it cannot decode.
    pub fn request_keyframe(&self) {
        let structure = gst::Structure::builder("GstForceKeyUnit")
            .field("all-headers", true)
            .build();
        if !self
            .encoder_element
            .send_event(gst::event::CustomUpstream::new(structure))
        {
            debug!("encoder did not accept the force-keyframe request");
        }
    }

    pub fn stats(&self) -> &Arc<VideoStats> {
        &self.stats
    }

    /// `(capture, videorate, encoded)` window: each is `(frames, gaps, max_gap_us)`.
    pub fn take_pacing(&self) -> (PaceWindow, PaceWindow, PaceWindow) {
        (
            self.capture_pace.take(),
            self.rate_pace.take(),
            self.encode_pace.take(),
        )
    }

    pub fn encoder(&self) -> Encoder {
        self.encoder
    }

    pub fn config(&self) -> &VideoConfig {
        &self.config
    }

    /// Logs asynchronous pipeline errors, which otherwise surface only as a
    /// silently stalled stream.
    fn watch_bus(&self) {
        let Some(bus) = self.pipeline.bus() else {
            return;
        };
        std::thread::spawn(move || {
            for msg in bus.iter_timed(gst::ClockTime::NONE) {
                match msg.view() {
                    gst::MessageView::Error(e) => {
                        error!(
                            source = ?e.src().map(|s| s.path_string()),
                            error = %e.error(),
                            debug = ?e.debug(),
                            "pipeline error"
                        );
                        break;
                    }
                    gst::MessageView::Warning(w) => {
                        warn!(warning = %w.error(), "pipeline warning");
                    }
                    gst::MessageView::Eos(_) => {
                        info!("pipeline reached end of stream");
                        break;
                    }
                    _ => {}
                }
            }
        });
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        self.stop();
    }
}
