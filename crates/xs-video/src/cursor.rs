//! Capture mutter's PipeWire screen-cast and split out the cursor.
//!
//! Embedded cursor mode only paints the pointer when the virtual monitor is
//! damaged, so a still window (or empty wallpaper) freezes it. Metadata mode
//! sends the sprite out-of-band, including on cursor-only buffers (`chunk` size
//! 0). `pipewiresrc` drops those, and a second consumer on the same node makes
//! it abort (`handle_format_change` with unfixed caps). This crate is therefore
//! the only PipeWire client: one stream reads frames and `SPA_META_Cursor`.
//!
//! Video damage is pushed into the encoder `appsrc`. Cursor motion is *not*
//! republished as a video frame -- it goes out on the control channel so the
//! tablet can composite an overlay. A cursor-only buffer therefore costs a
//! sprite/position message, not a convert+encode+USB cycle.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Instant;

use gstreamer as gst;
use gstreamer_app::AppSrc;
use pipewire::{self as pw, properties::properties, spa, sys as pw_sys};
use tokio::sync::mpsc;
use tracing::{debug, info, warn};
use xs_proto::{CursorBitmap, CursorMessage};

const CURSOR_META_MAX: u32 = 384;

#[derive(Clone, Default)]
struct Sprite {
    visible: bool,
    x: i32,
    y: i32,
    hot_x: i32,
    hot_y: i32,
    width: u32,
    height: u32,
    /// Packed BGRA, `width * 4` bytes per row.
    pixels: Vec<u8>,
}

struct VideoFrame {
    width: u32,
    height: u32,
    /// Packed BGRx, `width * 4` bytes per row.
    pixels: Vec<u8>,
}

#[derive(Default)]
struct PendingCursor {
    visible: bool,
    x: i32,
    y: i32,
    hot_x: i32,
    hot_y: i32,
    bitmap: Option<(u16, u16, Vec<u8>)>,
    pos_changed: bool,
    shape_changed: bool,
    vis_changed: bool,
}

pub struct CursorHub {
    sprite: Mutex<Sprite>,
    pending: Mutex<PendingCursor>,
    cursor_tx: mpsc::Sender<()>,
    appsrc: Mutex<Option<AppSrc>>,
    origin: Instant,
    framerate: u32,
    seen_cursor: AtomicBool,
    seen_video: AtomicBool,
}

pub struct Capture {
    loop_ptr: usize,
    join: Option<JoinHandle<()>>,
}

struct CaptureData {
    hub: Arc<CursorHub>,
    format: spa::param::video::VideoInfoRaw,
    warned_unmapped: bool,
    warned_format: bool,
}

impl CursorHub {
    pub fn new(framerate: u32, cursor_tx: mpsc::Sender<()>) -> Arc<Self> {
        Arc::new(Self {
            sprite: Mutex::new(Sprite::default()),
            pending: Mutex::new(PendingCursor::default()),
            cursor_tx,
            appsrc: Mutex::new(None),
            origin: Instant::now(),
            framerate: framerate.max(1),
            seen_cursor: AtomicBool::new(false),
            seen_video: AtomicBool::new(false),
        })
    }

    pub fn attach_appsrc(&self, appsrc: AppSrc) {
        *self.appsrc.lock().expect("cursor appsrc lock") = Some(appsrc);
    }

    pub fn take_cursor_message(&self) -> Option<CursorMessage> {
        let mut pending = self.pending.lock().expect("cursor pending lock");
        if !pending.pos_changed && !pending.shape_changed && !pending.vis_changed {
            return None;
        }
        let bitmap = if pending.shape_changed {
            pending
                .bitmap
                .take()
                .map(|(width, height, pixels)| CursorBitmap {
                    width,
                    height,
                    pixels,
                })
        } else {
            None
        };
        let hotspot = if pending.shape_changed {
            Some((pending.hot_x as i16, pending.hot_y as i16))
        } else {
            None
        };
        let position = if pending.pos_changed || pending.shape_changed {
            Some((pending.x, pending.y))
        } else {
            None
        };
        let msg = if pending.visible {
            CursorMessage {
                visible: true,
                position,
                hotspot,
                bitmap,
            }
        } else {
            CursorMessage::hide()
        };
        pending.pos_changed = false;
        pending.shape_changed = false;
        pending.vis_changed = false;
        Some(msg)
    }

    fn store_cursor(&self, update: CursorUpdate) {
        let mut sprite = self.sprite.lock().expect("cursor sprite lock");
        let emit = match update {
            CursorUpdate::Hidden => {
                if !sprite.visible && sprite.pixels.is_empty() {
                    None
                } else {
                    sprite.visible = false;
                    Some(CursorEmit::Hide)
                }
            }
            CursorUpdate::Move { x, y } => {
                if sprite.x == x && sprite.y == y && sprite.visible {
                    None
                } else {
                    sprite.x = x;
                    sprite.y = y;
                    sprite.visible = !sprite.pixels.is_empty();
                    if sprite.visible {
                        Some(CursorEmit::Move { x, y })
                    } else {
                        Some(CursorEmit::Hide)
                    }
                }
            }
            CursorUpdate::Bitmap(next) => {
                let same_shape = sprite.width == next.width
                    && sprite.height == next.height
                    && sprite.hot_x == next.hot_x
                    && sprite.hot_y == next.hot_y
                    && sprite.pixels == next.pixels;
                if same_shape {
                    if sprite.x == next.x && sprite.y == next.y && sprite.visible {
                        None
                    } else {
                        sprite.x = next.x;
                        sprite.y = next.y;
                        sprite.visible = true;
                        Some(CursorEmit::Move {
                            x: next.x,
                            y: next.y,
                        })
                    }
                } else {
                    *sprite = next;
                    Some(CursorEmit::Shape)
                }
            }
        };
        let snapshot = match &emit {
            Some(CursorEmit::Shape) => Some(sprite.clone()),
            _ => None,
        };
        drop(sprite);
        let Some(emit) = emit else {
            return;
        };
        if !self.seen_cursor.swap(true, Ordering::Relaxed) {
            info!("cursor metadata is arriving from mutter");
        }
        match emit {
            CursorEmit::Hide => self.queue_hide(),
            CursorEmit::Move { x, y } => self.queue_move(x, y),
            CursorEmit::Shape => {
                if let Some(sprite) = snapshot {
                    self.queue_shape(&sprite);
                }
            }
        }
    }

    fn queue_hide(&self) {
        let mut pending = self.pending.lock().expect("cursor pending lock");
        pending.visible = false;
        pending.vis_changed = true;
        drop(pending);
        let _ = self.cursor_tx.try_send(());
    }

    fn queue_move(&self, x: i32, y: i32) {
        let mut pending = self.pending.lock().expect("cursor pending lock");
        pending.visible = true;
        pending.x = x;
        pending.y = y;
        pending.pos_changed = true;
        drop(pending);
        let _ = self.cursor_tx.try_send(());
    }

    fn queue_shape(&self, sprite: &Sprite) {
        let mut pending = self.pending.lock().expect("cursor pending lock");
        pending.visible = sprite.visible;
        pending.x = sprite.x;
        pending.y = sprite.y;
        pending.hot_x = sprite.hot_x;
        pending.hot_y = sprite.hot_y;
        let width = sprite.width.min(u16::MAX as u32) as u16;
        let height = sprite.height.min(u16::MAX as u32) as u16;
        pending.bitmap = Some((width, height, sprite.pixels.clone()));
        pending.pos_changed = true;
        pending.shape_changed = true;
        pending.vis_changed = true;
        drop(pending);
        let _ = self.cursor_tx.try_send(());
    }

    fn on_video_frame(&self, frame: VideoFrame) {
        if !self.seen_video.swap(true, Ordering::Relaxed) {
            info!(
                width = frame.width,
                height = frame.height,
                "screen-cast frame arrived"
            );
            self.set_appsrc_caps(frame.width, frame.height);
        }
        self.push_frame(frame);
    }

    fn set_appsrc_caps(&self, width: u32, height: u32) {
        let Some(appsrc) = self.appsrc.lock().expect("cursor appsrc lock").clone() else {
            return;
        };
        let caps = gst::Caps::builder("video/x-raw")
            .field("format", "BGRx")
            .field("width", width as i32)
            .field("height", height as i32)
            .field("framerate", gst::Fraction::new(self.framerate as i32, 1))
            .build();
        appsrc.set_caps(Some(&caps));
    }

    fn push_frame(&self, frame: VideoFrame) {
        let Some(appsrc) = self.appsrc.lock().expect("cursor appsrc lock").clone() else {
            return;
        };
        let mut buffer = gst::Buffer::from_mut_slice(frame.pixels);
        {
            let buffer = buffer.get_mut().expect("new capture buffer is writable");
            buffer.set_pts(self.now_pts());
            buffer.set_duration(gst::ClockTime::from_nseconds(
                1_000_000_000 / u64::from(self.framerate),
            ));
        }
        if let Err(err) = appsrc.push_buffer(buffer) {
            debug!(error = %err, "capture push dropped");
        }
    }

    fn now_pts(&self) -> gst::ClockTime {
        gst::ClockTime::from_nseconds(self.origin.elapsed().as_nanos() as u64)
    }
}

enum CursorUpdate {
    Hidden,
    Move { x: i32, y: i32 },
    Bitmap(Sprite),
}

enum CursorEmit {
    Hide,
    Move { x: i32, y: i32 },
    Shape,
}

impl Capture {
    pub fn start(
        node_id: u32,
        width: u32,
        height: u32,
        hub: Arc<CursorHub>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("xs-capture".into())
            .spawn(move || capture_thread(node_id, width, height, hub, ready_tx))
            .map_err(|e| e.to_string())?;
        let loop_ptr = ready_rx
            .recv_timeout(std::time::Duration::from_secs(3))
            .map_err(|e| format!("capture thread did not start: {e}"))?;
        if loop_ptr == 0 {
            return Err("could not create a PipeWire loop for screen-cast capture".into());
        }
        Ok(Self {
            loop_ptr,
            join: Some(join),
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        if self.loop_ptr != 0 {
            unsafe {
                pw_sys::pw_main_loop_quit(self.loop_ptr as *mut pw_sys::pw_main_loop);
            }
        }
        if let Some(join) = self.join.take() {
            let _ = join.join();
        }
    }
}

fn capture_thread(
    node_id: u32,
    width: u32,
    height: u32,
    hub: Arc<CursorHub>,
    ready: std::sync::mpsc::Sender<usize>,
) {
    pw::init();
    let mainloop = match pw::main_loop::MainLoopRc::new(None) {
        Ok(loop_) => loop_,
        Err(e) => {
            warn!(error = %e, "could not create a PipeWire loop for screen-cast capture");
            let _ = ready.send(0);
            return;
        }
    };
    let loop_ptr = mainloop.as_raw_ptr() as usize;
    let _ = ready.send(loop_ptr);

    if let Err(e) = run_capture_stream(&mainloop, node_id, width, height, hub) {
        warn!(error = %e, "screen-cast stream failed");
    }
}

fn run_capture_stream(
    mainloop: &pw::main_loop::MainLoopRc,
    node_id: u32,
    width: u32,
    height: u32,
    hub: Arc<CursorHub>,
) -> Result<(), String> {
    let context = pw::context::ContextRc::new(mainloop, None).map_err(|e| e.to_string())?;
    let core = context.connect_rc(None).map_err(|e| e.to_string())?;
    let props = properties! {
        *pw::keys::MEDIA_TYPE => "Video",
        *pw::keys::MEDIA_CATEGORY => "Capture",
        *pw::keys::MEDIA_ROLE => "Screen",
    };
    let stream = pw::stream::StreamBox::new(&core, "extraspace-capture", props)
        .map_err(|e| e.to_string())?;

    let data = CaptureData {
        hub,
        format: spa::param::video::VideoInfoRaw::new(),
        warned_unmapped: false,
        warned_format: false,
    };

    let _listener = stream
        .add_local_listener_with_user_data(data)
        .state_changed(|_, _, old, new| {
            if matches!(new, pw::stream::StreamState::Error(_)) {
                warn!(?old, ?new, "screen-cast stream error");
            } else {
                debug!(?old, ?new, "screen-cast stream state");
            }
        })
        .param_changed(|_, data, id, param| {
            let Some(param) = param else {
                return;
            };
            if id != spa::param::ParamType::Format.as_raw() {
                return;
            }
            let (media_type, media_subtype) = match spa::param::format_utils::parse_format(param) {
                Ok(v) => v,
                Err(_) => return,
            };
            if media_type != spa::param::format::MediaType::Video
                || media_subtype != spa::param::format::MediaSubtype::Raw
            {
                return;
            }
            if data.format.parse(param).is_err() {
                return;
            }
            info!(
                format = ?data.format.format(),
                width = data.format.size().width,
                height = data.format.size().height,
                "screen-cast format negotiated"
            );
        })
        .process(|stream, data| {
            let raw = unsafe { stream.dequeue_raw_buffer() };
            if raw.is_null() {
                return;
            }
            let cursor = unsafe { parse_cursor(raw) };
            let video = unsafe { copy_video_frame(raw, data) };
            if let Some(update) = cursor {
                data.hub.store_cursor(update);
            }
            if let Some(frame) = video {
                data.hub.on_video_frame(frame);
            }
            unsafe { stream.queue_raw_buffer(raw) };
        })
        .register()
        .map_err(|e| e.to_string())?;

    let format_bytes = video_enum_format_pod(width, height);
    let meta_bytes = cursor_meta_pod();
    let buffers_bytes = cpu_buffers_pod();
    let format_pod = spa::pod::Pod::from_bytes(&format_bytes).ok_or("video format pod")?;
    let meta_pod = spa::pod::Pod::from_bytes(&meta_bytes).ok_or("cursor meta pod")?;
    let buffers_pod = spa::pod::Pod::from_bytes(&buffers_bytes).ok_or("buffers pod")?;
    let mut params = [format_pod, meta_pod, buffers_pod];
    stream
        .connect(
            spa::utils::Direction::Input,
            Some(node_id),
            pw::stream::StreamFlags::AUTOCONNECT
                | pw::stream::StreamFlags::MAP_BUFFERS
                | pw::stream::StreamFlags::DONT_RECONNECT,
            &mut params,
        )
        .map_err(|e| e.to_string())?;

    info!(node_id, width, height, "screen-cast stream connected");
    mainloop.run();
    Ok(())
}

fn video_enum_format_pod(width: u32, height: u32) -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::param::video::VideoFormat;
    use spa::pod::{object, property};

    let obj = object!(
        spa::utils::SpaTypes::ObjectParamFormat,
        spa::param::ParamType::EnumFormat,
        property!(FormatProperties::MediaType, Id, MediaType::Video),
        property!(FormatProperties::MediaSubtype, Id, MediaSubtype::Raw),
        property!(
            FormatProperties::VideoFormat,
            Choice,
            Enum,
            Id,
            VideoFormat::BGRx,
            VideoFormat::BGRx,
            VideoFormat::BGRA,
            VideoFormat::RGBx,
            VideoFormat::RGBA,
            VideoFormat::xRGB,
            VideoFormat::ARGB,
        ),
        property!(
            FormatProperties::VideoSize,
            Choice,
            Range,
            Rectangle,
            spa::utils::Rectangle { width, height },
            spa::utils::Rectangle {
                width: 1,
                height: 1
            },
            spa::utils::Rectangle {
                width: 8192,
                height: 8192
            }
        ),
        property!(
            FormatProperties::VideoFramerate,
            Choice,
            Range,
            Fraction,
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction { num: 0, denom: 1 },
            spa::utils::Fraction {
                num: 1000,
                denom: 1
            }
        ),
    );
    serialize_pod(spa::pod::Value::Object(obj))
}

fn cpu_buffers_pod() -> Vec<u8> {
    use spa::pod::{Object, Property, Value};

    let types = (1 << spa::sys::SPA_DATA_MemPtr) | (1 << spa::sys::SPA_DATA_MemFd);
    let obj = Object {
        type_: spa::utils::SpaTypes::ObjectParamBuffers.as_raw(),
        id: spa::param::ParamType::Buffers.as_raw(),
        properties: vec![Property::new(
            spa::sys::SPA_PARAM_BUFFERS_dataType,
            Value::Int(types),
        )],
    };
    serialize_pod(spa::pod::Value::Object(obj))
}

fn cursor_meta_pod() -> Vec<u8> {
    use spa::pod::{ChoiceValue, Object, Property, Value};
    use spa::utils::{Choice, ChoiceEnum, ChoiceFlags, Id};

    fn cursor_meta_size(w: u32, h: u32) -> i32 {
        (std::mem::size_of::<SpaMetaCursor>()
            + std::mem::size_of::<SpaMetaBitmap>()
            + (w * h * 4) as usize) as i32
    }

    let obj = Object {
        type_: spa::utils::SpaTypes::ObjectParamMeta.as_raw(),
        id: spa::param::ParamType::Meta.as_raw(),
        properties: vec![
            Property::new(
                spa::sys::SPA_PARAM_META_type,
                Value::Id(Id(spa::sys::SPA_META_Cursor)),
            ),
            Property::new(
                spa::sys::SPA_PARAM_META_size,
                Value::Choice(ChoiceValue::Int(Choice(
                    ChoiceFlags::empty(),
                    ChoiceEnum::Range {
                        default: cursor_meta_size(64, 64),
                        min: std::mem::size_of::<SpaMetaCursor>() as i32,
                        max: cursor_meta_size(CURSOR_META_MAX, CURSOR_META_MAX),
                    },
                ))),
            ),
        ],
    };
    serialize_pod(spa::pod::Value::Object(obj))
}

fn serialize_pod(value: spa::pod::Value) -> Vec<u8> {
    spa::pod::serialize::PodSerializer::serialize(std::io::Cursor::new(Vec::new()), &value)
        .expect("pod serialise")
        .0
        .into_inner()
}

#[repr(C)]
struct SpaMetaCursor {
    id: u32,
    flags: u32,
    x: i32,
    y: i32,
    hot_x: i32,
    hot_y: i32,
    bitmap_offset: u32,
}

#[repr(C)]
struct SpaMetaBitmap {
    format: u32,
    width: u32,
    height: u32,
    stride: i32,
    offset: u32,
}

unsafe fn parse_cursor(pw_buf: *mut pw_sys::pw_buffer) -> Option<CursorUpdate> {
    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() {
        return None;
    }
    let n_metas = (*spa_buf).n_metas;
    let metas = (*spa_buf).metas;
    if metas.is_null() {
        return None;
    }
    for i in 0..n_metas {
        let meta = metas.add(i as usize);
        if (*meta).type_ != spa::sys::SPA_META_Cursor {
            continue;
        }
        if (*meta).data.is_null() || (*meta).size < std::mem::size_of::<SpaMetaCursor>() as u32 {
            continue;
        }
        let cursor = &*((*meta).data as *const SpaMetaCursor);
        if cursor.id == 0 {
            return Some(CursorUpdate::Hidden);
        }
        if cursor.bitmap_offset < std::mem::size_of::<SpaMetaCursor>() as u32 {
            return Some(CursorUpdate::Move {
                x: cursor.x,
                y: cursor.y,
            });
        }
        let bitmap = &*((*meta).data as *const u8)
            .add(cursor.bitmap_offset as usize)
            .cast::<SpaMetaBitmap>();
        if bitmap.format == 0 || bitmap.width == 0 || bitmap.height == 0 || bitmap.offset == 0 {
            return Some(CursorUpdate::Hidden);
        }
        let pixels = cursor_pixels(bitmap);
        if pixels.is_empty() {
            return Some(CursorUpdate::Hidden);
        }
        return Some(CursorUpdate::Bitmap(Sprite {
            visible: true,
            x: cursor.x,
            y: cursor.y,
            hot_x: cursor.hot_x,
            hot_y: cursor.hot_y,
            width: bitmap.width,
            height: bitmap.height,
            pixels,
        }));
    }
    None
}

unsafe fn copy_video_frame(
    pw_buf: *mut pw_sys::pw_buffer,
    data: &mut CaptureData,
) -> Option<VideoFrame> {
    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() || (*spa_buf).n_datas == 0 || (*spa_buf).datas.is_null() {
        return None;
    }
    let spa_data = (*spa_buf).datas;
    if (*spa_data).chunk.is_null() {
        return None;
    }
    let chunk = *(*spa_data).chunk;
    if chunk.size == 0 {
        return None;
    }
    if (*spa_data).data.is_null() {
        if !data.warned_unmapped {
            data.warned_unmapped = true;
            warn!(
                data_type = (*spa_data).type_,
                "screen-cast buffer is not CPU-mapped; cursor overlay cannot read frames"
            );
        }
        return None;
    }
    let width = data.format.size().width;
    let height = data.format.size().height;
    if width == 0 || height == 0 {
        return None;
    }
    let mapped =
        std::slice::from_raw_parts((*spa_data).data as *const u8, (*spa_data).maxsize as usize);
    let offset = chunk.offset as usize;
    let size = chunk.size as usize;
    if offset
        .checked_add(size)
        .is_none_or(|end| end > mapped.len())
    {
        return None;
    }
    let pixels = &mapped[offset..offset + size];
    let stride = if chunk.stride > 0 {
        chunk.stride as usize
    } else {
        (width * 4) as usize
    };
    match pack_video(pixels, stride, width, height, data.format.format()) {
        Some(packed) => Some(VideoFrame {
            width,
            height,
            pixels: packed,
        }),
        None => {
            if !data.warned_format {
                data.warned_format = true;
                warn!(
                    format = ?data.format.format(),
                    width,
                    height,
                    stride,
                    "unsupported screen-cast pixel format"
                );
            }
            None
        }
    }
}

unsafe fn cursor_pixels(bitmap: &SpaMetaBitmap) -> Vec<u8> {
    let src = (bitmap as *const SpaMetaBitmap as *const u8).add(bitmap.offset as usize);
    let src_stride = bitmap.stride as usize;
    let dst_stride = (bitmap.width * 4) as usize;
    let height = bitmap.height as usize;
    let mut out = vec![0u8; dst_stride * height];
    for y in 0..height {
        let row = std::slice::from_raw_parts(src.add(y * src_stride), dst_stride.min(src_stride));
        let dest = &mut out[y * dst_stride..y * dst_stride + row.len()];
        let convert = match bitmap.format {
            f if f == spa::sys::SPA_VIDEO_FORMAT_BGRA || f == spa::sys::SPA_VIDEO_FORMAT_BGRx => {
                copy_bgra
            }
            f if f == spa::sys::SPA_VIDEO_FORMAT_RGBA || f == spa::sys::SPA_VIDEO_FORMAT_RGBx => {
                swap_rb
            }
            f if f == spa::sys::SPA_VIDEO_FORMAT_ARGB => argb_to_bgrx,
            other => {
                debug!(format = other, "unsupported cursor bitmap format");
                return Vec::new();
            }
        };
        convert(row, dest);
    }
    out
}

fn pack_video(
    src: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    format: spa::param::video::VideoFormat,
) -> Option<Vec<u8>> {
    use spa::param::video::VideoFormat;
    match format {
        VideoFormat::BGRx | VideoFormat::BGRA => pack_rows(src, stride, width, height, copy_bgra),
        VideoFormat::RGBx | VideoFormat::RGBA => pack_rows(src, stride, width, height, swap_rb),
        VideoFormat::xRGB | VideoFormat::ARGB => {
            pack_rows(src, stride, width, height, argb_to_bgrx)
        }
        _ => None,
    }
}

fn pack_rows(
    src: &[u8],
    stride: usize,
    width: u32,
    height: u32,
    convert: fn(&[u8], &mut [u8]),
) -> Option<Vec<u8>> {
    let row = (width * 4) as usize;
    if stride < row {
        return None;
    }
    let mut out = vec![0u8; row * height as usize];
    for y in 0..height as usize {
        let start = y * stride;
        if start + row > src.len() {
            return None;
        }
        convert(&src[start..start + row], &mut out[y * row..(y + 1) * row]);
    }
    Some(out)
}

fn copy_bgra(src: &[u8], dest: &mut [u8]) {
    dest.copy_from_slice(src);
}

fn swap_rb(src: &[u8], dest: &mut [u8]) {
    for (px, dest) in src.chunks_exact(4).zip(dest.chunks_exact_mut(4)) {
        dest[0] = px[2];
        dest[1] = px[1];
        dest[2] = px[0];
        dest[3] = px[3];
    }
}

fn argb_to_bgrx(src: &[u8], dest: &mut [u8]) {
    for (px, dest) in src.chunks_exact(4).zip(dest.chunks_exact_mut(4)) {
        dest[0] = px[3];
        dest[1] = px[2];
        dest[2] = px[1];
        dest[3] = px[0];
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_hub() -> (Arc<CursorHub>, mpsc::Receiver<()>) {
        let (tx, rx) = mpsc::channel(1);
        (CursorHub::new(60, tx), rx)
    }

    fn sample_sprite(x: i32, y: i32, pixel: u8) -> Sprite {
        Sprite {
            visible: true,
            x,
            y,
            hot_x: 1,
            hot_y: 2,
            width: 1,
            height: 1,
            pixels: vec![pixel, 0, 0, 255],
        }
    }

    #[test]
    fn latest_cursor_position_wins() {
        let (hub, mut rx) = test_hub();
        hub.store_cursor(CursorUpdate::Bitmap(sample_sprite(10, 20, 9)));
        hub.store_cursor(CursorUpdate::Move { x: 11, y: 21 });
        hub.store_cursor(CursorUpdate::Move { x: 30, y: 40 });
        let msg = hub.take_cursor_message().unwrap();
        assert!(msg.visible);
        assert_eq!(msg.position, Some((30, 40)));
        assert!(msg.bitmap.is_some());
        assert!(rx.try_recv().is_ok());
        assert!(hub.take_cursor_message().is_none());
    }

    #[test]
    fn unchanged_sprite_is_position_only() {
        let (hub, _) = test_hub();
        hub.store_cursor(CursorUpdate::Bitmap(sample_sprite(10, 20, 9)));
        let _ = hub.take_cursor_message();
        hub.store_cursor(CursorUpdate::Bitmap(sample_sprite(15, 25, 9)));
        let msg = hub.take_cursor_message().unwrap();
        assert_eq!(msg.position, Some((15, 25)));
        assert!(msg.bitmap.is_none());
        assert!(msg.hotspot.is_none());
    }

    #[test]
    fn rgbx_frame_is_packed_as_bgrx() {
        let src = [1u8, 2, 3, 4];
        let packed = pack_video(&src, 4, 1, 1, spa::param::video::VideoFormat::RGBx).unwrap();
        assert_eq!(packed, vec![3, 2, 1, 4]);
    }
}
