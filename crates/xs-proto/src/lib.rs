//! The extraspace wire protocol, shared by the Linux daemon and the Android app.
//!
//! Three separate sockets are used rather than one multiplexed stream. Touch events
//! are tiny and latency-critical; video is a steady ~15 Mbit/s. Sharing one socket
//! would queue a touch behind whatever video frame is in flight, so they are kept
//! apart and the channel byte exists mainly for sanity-checking and logging.
//!
//! Every frame carries a fixed 20-byte header, little-endian:
//!
//! ```text
//!  0..4   magic  u32   always MAGIC
//!  4      channel u8   Channel
//!  5      kind    u8   message kind, interpreted per-channel
//!  6..8   flags   u16  Flags bitfield
//!  8..12  len     u32  payload length
//! 12..20  pts_us  u64  presentation timestamp, microseconds
//! ```
//!
//! The Kotlin side mirrors this in `Protocol.kt`; changing either without the other
//! will fail the [`MAGIC`] check on the first frame rather than corrupt silently.

use serde::{Deserialize, Serialize};

/// Reads as the ASCII bytes `XSPA` on the wire (little-endian).
pub const MAGIC: u32 = 0x4150_5358;

/// Bytes in a frame header.
pub const HEADER_LEN: usize = 20;

/// Refuse absurd frames early rather than trying to allocate them.
pub const MAX_PAYLOAD: u32 = 16 * 1024 * 1024;

/// Default TCP ports, forwarded over adb. Chosen to sit just above scrcpy's 27183
/// so the two can run side by side.
pub mod ports {
    pub const CONTROL: u16 = 27183;
    pub const VIDEO: u16 = 27184;
    pub const CAMERA: u16 = 27185;
}

/// Which logical stream a frame belongs to.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[repr(u8)]
pub enum Channel {
    Control = 0,
    Touch = 1,
    VideoDown = 2,
    CameraUp = 3,
}

impl Channel {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Control,
            1 => Self::Touch,
            2 => Self::VideoDown,
            3 => Self::CameraUp,
            _ => return None,
        })
    }
}

/// Frame flags.
pub mod flags {
    /// Payload is a keyframe (IDR). Set on video/camera frames.
    pub const KEYFRAME: u16 = 1 << 0;
    /// Payload is codec configuration (SPS/PPS), not a displayable frame.
    pub const CODEC_CONFIG: u16 = 1 << 1;
}

/// Message kinds on [`Channel::Control`].
///
/// Hello / VideoConfig / Stats / CameraControl / Error are JSON: they are rare
/// and being able to read them in a log is worth more than the bytes saved.
/// [`ControlKind::Cursor`] is binary -- it can arrive at the panel refresh rate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ControlKind {
    /// Device -> host, first message: identifies the tablet.
    Hello = 0,
    /// Host -> device: display stream is about to start with these parameters.
    VideoConfig = 1,
    /// Device -> host, periodic: decode health, drives adaptive bitrate.
    Stats = 2,
    /// Host -> device: begin/end camera capture.
    CameraControl = 3,
    /// Either direction: liveness probe, echoed back with the same `pts_us`.
    Ping = 4,
    Pong = 5,
    /// Either direction: fatal error, connection is about to close.
    Error = 6,
    /// Host -> device: cursor overlay. Binary; see [`CursorMessage`].
    Cursor = 7,
}

impl ControlKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Hello,
            1 => Self::VideoConfig,
            2 => Self::Stats,
            3 => Self::CameraControl,
            4 => Self::Ping,
            5 => Self::Pong,
            6 => Self::Error,
            7 => Self::Cursor,
            _ => return None,
        })
    }
}

/// Flags for [`CursorMessage`].
pub mod cursor_flags {
    pub const VISIBLE: u8 = 1 << 0;
    pub const POSITION: u8 = 1 << 1;
    pub const HOTSPOT: u8 = 1 << 2;
    pub const BITMAP: u8 = 1 << 3;
}

/// Packed BGRA cursor sprite, `width * 4` bytes per row.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    pub width: u16,
    pub height: u16,
    pub pixels: Vec<u8>,
}

/// Host -> device cursor overlay update.
///
/// ```text
/// 0      flags  u8    cursor_flags
/// [if POSITION] x i32  y i32     hotspot location in stream pixels
/// [if HOTSPOT]  hx i16 hy i16    hotspot offset inside the sprite
/// [if BITMAP]   w u16  h u16     then w*h*4 BGRA pixels
/// ```
///
/// Position-only updates must not carry a bitmap. The tablet keeps the last
/// sprite until a new one arrives or the cursor is hidden.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMessage {
    pub visible: bool,
    pub position: Option<(i32, i32)>,
    pub hotspot: Option<(i16, i16)>,
    pub bitmap: Option<CursorBitmap>,
}

impl CursorMessage {
    pub fn hide() -> Self {
        Self {
            visible: false,
            position: None,
            hotspot: None,
            bitmap: None,
        }
    }

    pub fn encode(&self) -> Vec<u8> {
        let mut flags = 0u8;
        if self.visible {
            flags |= cursor_flags::VISIBLE;
        }
        if self.position.is_some() {
            flags |= cursor_flags::POSITION;
        }
        if self.hotspot.is_some() {
            flags |= cursor_flags::HOTSPOT;
        }
        if self.bitmap.is_some() {
            flags |= cursor_flags::BITMAP;
        }
        let mut len = 1;
        if self.position.is_some() {
            len += 8;
        }
        if self.hotspot.is_some() {
            len += 4;
        }
        if let Some(bitmap) = &self.bitmap {
            len += 4 + bitmap.pixels.len();
        }
        let mut buf = Vec::with_capacity(len);
        buf.push(flags);
        if let Some((x, y)) = self.position {
            buf.extend_from_slice(&x.to_le_bytes());
            buf.extend_from_slice(&y.to_le_bytes());
        }
        if let Some((hx, hy)) = self.hotspot {
            buf.extend_from_slice(&hx.to_le_bytes());
            buf.extend_from_slice(&hy.to_le_bytes());
        }
        if let Some(bitmap) = &self.bitmap {
            buf.extend_from_slice(&bitmap.width.to_le_bytes());
            buf.extend_from_slice(&bitmap.height.to_le_bytes());
            buf.extend_from_slice(&bitmap.pixels);
        }
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.is_empty() {
            return None;
        }
        let flags = buf[0];
        let mut i = 1;
        let position = if flags & cursor_flags::POSITION != 0 {
            if i + 8 > buf.len() {
                return None;
            }
            let x = i32::from_le_bytes(buf[i..i + 4].try_into().ok()?);
            let y = i32::from_le_bytes(buf[i + 4..i + 8].try_into().ok()?);
            i += 8;
            Some((x, y))
        } else {
            None
        };
        let hotspot = if flags & cursor_flags::HOTSPOT != 0 {
            if i + 4 > buf.len() {
                return None;
            }
            let hx = i16::from_le_bytes(buf[i..i + 2].try_into().ok()?);
            let hy = i16::from_le_bytes(buf[i + 2..i + 4].try_into().ok()?);
            i += 4;
            Some((hx, hy))
        } else {
            None
        };
        let bitmap = if flags & cursor_flags::BITMAP != 0 {
            if i + 4 > buf.len() {
                return None;
            }
            let width = u16::from_le_bytes(buf[i..i + 2].try_into().ok()?);
            let height = u16::from_le_bytes(buf[i + 2..i + 4].try_into().ok()?);
            i += 4;
            let pixels_len = (width as usize)
                .checked_mul(height as usize)?
                .checked_mul(4)?;
            if i + pixels_len != buf.len() {
                return None;
            }
            Some(CursorBitmap {
                width,
                height,
                pixels: buf[i..].to_vec(),
            })
        } else if i != buf.len() {
            return None;
        } else {
            None
        };
        Some(Self {
            visible: flags & cursor_flags::VISIBLE != 0,
            position,
            hotspot,
            bitmap,
        })
    }
}

/// First message from the tablet. The host uses `width`/`height`/`density` to size
/// the virtual monitor to the panel exactly.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Hello {
    pub protocol_version: u32,
    pub device_name: String,
    pub android_release: String,
    pub width: u32,
    pub height: u32,
    pub density_dpi: u32,
    pub refresh_rate: f64,
    /// Camera ids the tablet can offer, e.g. `["0", "1"]`.
    pub cameras: Vec<CameraInfo>,
}

pub const PROTOCOL_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraInfo {
    pub id: String,
    /// `"back"`, `"front"` or `"external"`.
    pub facing: String,
    pub max_width: u32,
    pub max_height: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoConfig {
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
    /// Always `"h264"` for now; present so the tablet can reject what it cannot decode.
    pub codec: String,
}

/// Periodic health report that drives the adaptive controller.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Stats {
    /// Frames waiting in the decoder input queue. Sustained growth means the
    /// tablet cannot keep up and bitrate should come down.
    pub decode_queue_depth: u32,
    pub frames_decoded: u64,
    pub frames_dropped: u64,
    /// Device-side receive timestamp of the most recent frame, in `pts_us` terms.
    pub last_frame_pts_us: u64,
    /// Device clock when that frame was rendered, microseconds since boot.
    pub rendered_at_us: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CameraControl {
    pub enabled: bool,
    pub camera_id: String,
    pub width: u32,
    pub height: u32,
    pub framerate: u32,
    pub bitrate_kbps: u32,
}

/// A single touch point, sent on [`Channel::Touch`].
///
/// Encoded as a fixed 21-byte payload rather than JSON: these arrive at up to
/// 120 Hz per finger and the parse cost matters.
///
/// ```text
/// 0      action u8   TouchAction
/// 1..5   slot   u32
/// 5..13  x      f64  stream coordinates
/// 13..21 y      f64
/// ```
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct TouchEvent {
    pub action: TouchAction,
    pub slot: u32,
    pub x: f64,
    pub y: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchAction {
    Down = 0,
    Motion = 1,
    Up = 2,
}

impl TouchAction {
    pub fn from_u8(v: u8) -> Option<Self> {
        Some(match v {
            0 => Self::Down,
            1 => Self::Motion,
            2 => Self::Up,
            _ => return None,
        })
    }
}

pub const TOUCH_PAYLOAD_LEN: usize = 21;

impl TouchEvent {
    pub fn encode(&self) -> [u8; TOUCH_PAYLOAD_LEN] {
        let mut buf = [0u8; TOUCH_PAYLOAD_LEN];
        buf[0] = self.action as u8;
        buf[1..5].copy_from_slice(&self.slot.to_le_bytes());
        buf[5..13].copy_from_slice(&self.x.to_le_bytes());
        buf[13..21].copy_from_slice(&self.y.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Option<Self> {
        if buf.len() < TOUCH_PAYLOAD_LEN {
            return None;
        }
        Some(Self {
            action: TouchAction::from_u8(buf[0])?,
            slot: u32::from_le_bytes(buf[1..5].try_into().ok()?),
            x: f64::from_le_bytes(buf[5..13].try_into().ok()?),
            y: f64::from_le_bytes(buf[13..21].try_into().ok()?),
        })
    }
}

/// Parsed frame header.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Header {
    pub channel: Channel,
    pub kind: u8,
    pub flags: u16,
    pub len: u32,
    pub pts_us: u64,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ProtoError {
    #[error("bad magic {0:#010x}: peer is not speaking extraspace, or the stream desynced")]
    BadMagic(u32),
    #[error("unknown channel {0}")]
    UnknownChannel(u8),
    #[error("payload of {0} bytes exceeds the {MAX_PAYLOAD} byte limit")]
    PayloadTooLarge(u32),
    #[error("need {needed} bytes for a header, got {got}")]
    Short { needed: usize, got: usize },
}

impl Header {
    pub fn encode(&self) -> [u8; HEADER_LEN] {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&MAGIC.to_le_bytes());
        buf[4] = self.channel as u8;
        buf[5] = self.kind;
        buf[6..8].copy_from_slice(&self.flags.to_le_bytes());
        buf[8..12].copy_from_slice(&self.len.to_le_bytes());
        buf[12..20].copy_from_slice(&self.pts_us.to_le_bytes());
        buf
    }

    pub fn decode(buf: &[u8]) -> Result<Self, ProtoError> {
        if buf.len() < HEADER_LEN {
            return Err(ProtoError::Short {
                needed: HEADER_LEN,
                got: buf.len(),
            });
        }
        let magic = u32::from_le_bytes(buf[0..4].try_into().unwrap());
        if magic != MAGIC {
            return Err(ProtoError::BadMagic(magic));
        }
        let channel = Channel::from_u8(buf[4]).ok_or(ProtoError::UnknownChannel(buf[4]))?;
        let len = u32::from_le_bytes(buf[8..12].try_into().unwrap());
        if len > MAX_PAYLOAD {
            return Err(ProtoError::PayloadTooLarge(len));
        }
        Ok(Self {
            channel,
            kind: buf[5],
            flags: u16::from_le_bytes(buf[6..8].try_into().unwrap()),
            len,
            pts_us: u64::from_le_bytes(buf[12..20].try_into().unwrap()),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn header_roundtrips() {
        let h = Header {
            channel: Channel::VideoDown,
            kind: 7,
            flags: flags::KEYFRAME,
            len: 4096,
            pts_us: 1_234_567_890,
        };
        assert_eq!(Header::decode(&h.encode()).unwrap(), h);
    }

    #[test]
    fn header_rejects_foreign_data() {
        let mut buf = [0u8; HEADER_LEN];
        buf[0..4].copy_from_slice(&0xdead_beefu32.to_le_bytes());
        assert_eq!(Header::decode(&buf), Err(ProtoError::BadMagic(0xdead_beef)));
    }

    #[test]
    fn header_rejects_oversized_payload() {
        let h = Header {
            channel: Channel::Control,
            kind: 0,
            flags: 0,
            len: MAX_PAYLOAD + 1,
            pts_us: 0,
        };
        assert_eq!(
            Header::decode(&h.encode()),
            Err(ProtoError::PayloadTooLarge(MAX_PAYLOAD + 1))
        );
    }

    #[test]
    fn touch_roundtrips_with_subpixel_precision() {
        let t = TouchEvent {
            action: TouchAction::Motion,
            slot: 3,
            x: 1234.5678,
            y: 987.6543,
        };
        assert_eq!(TouchEvent::decode(&t.encode()).unwrap(), t);
    }

    #[test]
    fn magic_reads_as_xspa_on_the_wire() {
        assert_eq!(&MAGIC.to_le_bytes(), b"XSPA");
    }

    #[test]
    fn cursor_hide_is_one_zero_byte() {
        let msg = CursorMessage::hide();
        let bytes = msg.encode();
        assert_eq!(bytes, [0]);
        assert_eq!(CursorMessage::decode(&bytes).unwrap(), msg);
    }

    #[test]
    fn cursor_move_does_not_carry_a_bitmap() {
        let msg = CursorMessage {
            visible: true,
            position: Some((1316, 10)),
            hotspot: None,
            bitmap: None,
        };
        let decoded = CursorMessage::decode(&msg.encode()).unwrap();
        assert_eq!(decoded, msg);
        assert!(decoded.bitmap.is_none());
    }

    #[test]
    fn cursor_sprite_roundtrips() {
        let msg = CursorMessage {
            visible: true,
            position: Some((40, 50)),
            hotspot: Some((3, 4)),
            bitmap: Some(CursorBitmap {
                width: 2,
                height: 1,
                pixels: vec![1, 2, 3, 255, 4, 5, 6, 128],
            }),
        };
        assert_eq!(CursorMessage::decode(&msg.encode()).unwrap(), msg);
    }
}
