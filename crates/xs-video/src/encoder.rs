//! Picking an H.264 encoder at runtime.
//!
//! There is no single encoder we can rely on being present, and the differences
//! between them are not cosmetic -- they disagree on the *units* of the `bitrate`
//! property, which is an easy way to ship a stream 1000x the intended size.
//!
//! Measured on the development machine (i5-11400F / RTX 2060, 2000x1200 @ 60):
//!
//! | element         | fps  | bitrate honoured | ships with Fedora |
//! |-----------------|------|------------------|-------------------|
//! | `x264enc`       | 292  | yes (+-1%)       | no, plugins-ugly  |
//! | `openh264enc`   | 108  | yes              | yes               |
//! | `vulkanh264enc` | ~96  | **no**           | yes               |
//!
//! `vulkanh264enc` is deliberately excluded: it reports CBR support but produces
//! byte-identical output at 5, 15 and 40 Mbit/s, which makes adaptive control
//! impossible and pins the link at roughly 48 Mbit/s.

use gst::prelude::*;
use gstreamer as gst;
use tracing::{debug, info, warn};

/// An H.264 encoder we know how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Encoder {
    /// Preferred software encoder: fastest, most accurate rate control.
    X264,
    /// Fallback that ships with Fedora, so the app works with no extra installs.
    OpenH264,
    /// VA-API on the GPU's low-power (VDENC) engine.
    VaH264Lp,
    /// VA-API on the general encode engine, where VDENC is absent.
    VaH264,
}

impl Encoder {
    pub fn element_name(self) -> &'static str {
        match self {
            Self::X264 => "x264enc",
            Self::OpenH264 => "openh264enc",
            Self::VaH264Lp => "vah264lpenc",
            Self::VaH264 => "vah264enc",
        }
    }

    pub fn human_name(self) -> &'static str {
        match self {
            Self::X264 => "x264 (software, recommended)",
            Self::OpenH264 => "OpenH264 (software, fallback)",
            Self::VaH264Lp => "VA-API (GPU, low-power engine)",
            Self::VaH264 => "VA-API (GPU)",
        }
    }

    /// Whether this encoder consumes GPU surfaces rather than system memory.
    ///
    /// At panel resolution a frame is 13 MB, and the measured cost of the whole
    /// software chain is dominated by moving those bytes rather than by the
    /// encode itself, so a GPU encoder is only worth having if the frames reach
    /// it without a trip through the CPU.
    pub fn is_gpu(self) -> bool {
        matches!(self, Self::VaH264Lp | Self::VaH264)
    }

    /// Encoders in descending order of preference.
    pub const PREFERENCE: [Encoder; 2] = [Encoder::X264, Encoder::OpenH264];

    /// GPU encoders in descending order of preference. The low-power engine is
    /// first: it is a separate fixed-function block, so it leaves the render
    /// engine free for the compositing that produced the frame.
    pub const GPU_PREFERENCE: [Encoder; 2] = [Encoder::VaH264Lp, Encoder::VaH264];

    /// Whether this encoder's element is registered in the local GStreamer install.
    pub fn is_available(self) -> bool {
        gst::ElementFactory::find(self.element_name()).is_some()
    }

    /// Best encoder present on this machine.
    pub fn detect() -> Option<Self> {
        let found = Self::PREFERENCE.into_iter().find(|e| e.is_available());
        match found {
            Some(Self::X264) => info!("using x264enc"),
            Some(Self::OpenH264) => warn!(
                "x264enc not found, falling back to openh264enc. For lower CPU use and better \
                 quality install gstreamer1-plugins-ugly."
            ),
            // `PREFERENCE` holds only software encoders, so nothing else can
            // come out of this search.
            _ => {}
        }
        found
    }

    /// Best GPU encoder present, or `None` when there is no VA-API plugin.
    pub fn detect_gpu() -> Option<Self> {
        let found = Self::GPU_PREFERENCE.into_iter().find(|e| e.is_available());
        if let Some(encoder) = found {
            info!(element = encoder.element_name(), "using GPU encoder");
        }
        found
    }

    /// `bitrate` is kbit/s for x264enc and the VA encoders but bit/s for
    /// openh264enc. Getting this wrong is a 1000x error, so the conversion lives
    /// in exactly one place.
    fn bitrate_property_value(self, kbps: u32) -> u32 {
        match self {
            Self::X264 | Self::VaH264Lp | Self::VaH264 => kbps,
            Self::OpenH264 => kbps.saturating_mul(1000),
        }
    }

    /// Builds the encoder element configured for low-latency streaming.
    pub fn build(self, kbps: u32, framerate: u32) -> Result<gst::Element, gst::glib::BoolError> {
        let e = gst::ElementFactory::make(self.element_name());
        let element = match self {
            Self::X264 => e
                .property("bitrate", self.bitrate_property_value(kbps))
                // zerolatency disables lookahead and B-frames; without it the
                // encoder buffers frames and adds tens of milliseconds.
                .property_from_str("tune", "zerolatency")
                .property_from_str("speed-preset", "veryfast")
                // Sliced threads keep zerolatency's frame-in/frame-out; the
                // default thread pool can add a scheduling hitch on U-series CPUs.
                .property("sliced-threads", true)
                .property("threads", 2u32)
                // Annex-B, so the tablet can feed bytes straight to MediaCodec.
                .property_from_str("byte-stream", "true")
                // Keyframe interval is counted in *frames*, and an idle desktop
                // only produces ~11 fps, so a naive framerate*2 can mean ten
                // seconds between keyframes -- and a tablet that reconnects
                // stares at a black screen until the next one. Sized against the
                // idle rate instead, and the session explicitly asks for a
                // keyframe whenever a tablet attaches.
                .property("key-int-max", keyframe_interval_frames(framerate))
                .build()?,
            Self::OpenH264 => e
                .property("bitrate", self.bitrate_property_value(kbps))
                .property_from_str("rate-control", "bitrate")
                .property_from_str("complexity", "low")
                .property("gop-size", keyframe_interval_frames(framerate))
                .build()?,
            // Rate control is left at the element's default (CBR), which the
            // adaptive controller can steer predictably. B-frames would reorder
            // output and add a frame of latency for a display that is being
            // driven live, and one reference frame is all a screen cast needs.
            Self::VaH264Lp | Self::VaH264 => e
                .property("bitrate", self.bitrate_property_value(kbps))
                .property("key-int-max", keyframe_interval_frames(framerate))
                .property("b-frames", 0u32)
                .property("ref-frames", 1u32)
                .build()?,
        };
        debug!(
            encoder = self.element_name(),
            kbps, framerate, "encoder built"
        );
        Ok(element)
    }

    /// Changes bitrate on a running pipeline. Both supported encoders accept this
    /// in PLAYING state, which is what makes adaptive control possible.
    pub fn set_bitrate(self, element: &gst::Element, kbps: u32) {
        element.set_property("bitrate", self.bitrate_property_value(kbps));
    }
}

/// Keyframe interval, in frames, targeting roughly two seconds of *wall clock*.
///
/// Both encoders count this in frames, but the real frame rate depends entirely
/// on how much of the screen is changing: measured at ~11 fps on a still desktop
/// versus the nominal 60. Sizing against the nominal rate would mean a keyframe
/// only every ten seconds while idle, which is exactly when a tablet is most
/// likely to attach and need one.
fn keyframe_interval_frames(nominal_framerate: u32) -> u32 {
    /// Rate to assume when the screen is mostly static, measured on real hardware.
    const IDLE_FPS: u32 = 12;
    const TARGET_SECONDS: u32 = 2;
    nominal_framerate
        .min(IDLE_FPS)
        .saturating_mul(TARGET_SECONDS)
        .max(2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bitrate_units_differ_between_encoders() {
        // The whole reason this conversion is centralised.
        assert_eq!(Encoder::X264.bitrate_property_value(15_000), 15_000);
        assert_eq!(Encoder::OpenH264.bitrate_property_value(15_000), 15_000_000);
        // The VA encoders document their bitrate in kbps, like x264, so the
        // adaptive controller needs no special case for them.
        assert_eq!(Encoder::VaH264Lp.bitrate_property_value(15_000), 15_000);
        assert_eq!(Encoder::VaH264.bitrate_property_value(15_000), 15_000);
    }

    #[test]
    fn only_va_encoders_are_gpu() {
        assert!(Encoder::VaH264Lp.is_gpu());
        assert!(Encoder::VaH264.is_gpu());
        assert!(!Encoder::X264.is_gpu());
        assert!(!Encoder::OpenH264.is_gpu());
    }

    #[test]
    fn bitrate_conversion_cannot_overflow() {
        assert_eq!(Encoder::OpenH264.bitrate_property_value(u32::MAX), u32::MAX);
    }

    #[test]
    fn x264_is_preferred_over_openh264() {
        assert_eq!(Encoder::PREFERENCE[0], Encoder::X264);
    }

    #[test]
    fn keyframe_interval_assumes_the_idle_rate_not_the_nominal_one() {
        // A still desktop measured ~11 fps against a nominal 60. Using 60 would
        // put keyframes 10s apart, so this must not scale with the nominal rate.
        assert_eq!(keyframe_interval_frames(60), 24);
        assert_eq!(keyframe_interval_frames(30), 24);
        // Below the idle assumption, the real rate is the limit.
        assert_eq!(keyframe_interval_frames(10), 20);
    }

    #[test]
    fn keyframe_interval_is_never_degenerate() {
        for fps in [0, 1, 2, 5, 60, 240] {
            assert!(keyframe_interval_frames(fps) >= 2, "fps {fps}");
        }
    }
}
