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

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
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
    /// PipeWire process() callbacks that carried a video chunk.
    mutter_frames: AtomicU64,
    /// Frames successfully pushed into appsrc.
    pushed_frames: AtomicU64,
    /// appsrc rejected the buffer (downstream full).
    push_fail: AtomicU64,
    copy_max_us: AtomicU64,
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
    /// Skip packing video; used to isolate PipeWire dequeue from the CPU copy.
    dequeue_only: bool,
    /// The layout requested from mutter, when frames should arrive as dma-bufs.
    dmabuf: Option<DmaBufImport>,
    allocator: Option<gstreamer_allocators::DmaBufAllocator>,
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
            mutter_frames: AtomicU64::new(0),
            pushed_frames: AtomicU64::new(0),
            push_fail: AtomicU64::new(0),
            copy_max_us: AtomicU64::new(0),
        })
    }

    pub fn attach_appsrc(&self, appsrc: AppSrc) {
        *self.appsrc.lock().expect("cursor appsrc lock") = Some(appsrc);
    }

    /// `(mutter_frames, pushed, push_fail, copy_max_us)` since the previous take.
    pub fn take_capture_counts(&self) -> (u64, u64, u64, u64) {
        (
            self.mutter_frames.swap(0, Ordering::Relaxed),
            self.pushed_frames.swap(0, Ordering::Relaxed),
            self.push_fail.swap(0, Ordering::Relaxed),
            self.copy_max_us.swap(0, Ordering::Relaxed),
        )
    }

    pub fn observe_copy_us(&self, copy_us: u64) {
        let mut current = self.copy_max_us.load(Ordering::Relaxed);
        while copy_us > current {
            match self.copy_max_us.compare_exchange_weak(
                current,
                copy_us,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => break,
                Err(seen) => current = seen,
            }
        }
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

    /// Pushes a GPU frame straight through, with no pixel access on this side.
    fn on_dmabuf_frame(
        &self,
        buffer: gst::Buffer,
        width: u32,
        height: u32,
        drm_format: &str,
        stride: i32,
    ) {
        if !self.seen_video.swap(true, Ordering::Relaxed) {
            info!(
                width,
                height,
                drm_format,
                stride,
                unpadded = width * 4,
                "dma-buf frame arrived"
            );
            let caps = gst::Caps::builder("video/x-raw")
                .features(["memory:DMABuf"])
                .field("format", "DMA_DRM")
                .field("drm-format", drm_format)
                .field("width", width as i32)
                .field("height", height as i32)
                .field("framerate", gst::Fraction::new(self.framerate as i32, 1))
                .build();
            if let Some(appsrc) = self.appsrc.lock().expect("cursor appsrc lock").clone() {
                appsrc.set_caps(Some(&caps));
            }
        }
        self.push_buffer(buffer);
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
        self.push_buffer(gst::Buffer::from_mut_slice(frame.pixels));
    }

    fn push_buffer(&self, mut buffer: gst::Buffer) {
        let Some(appsrc) = self.appsrc.lock().expect("cursor appsrc lock").clone() else {
            return;
        };
        {
            let buffer = buffer.get_mut().expect("new capture buffer is writable");
            buffer.set_pts(self.now_pts());
            buffer.set_duration(gst::ClockTime::from_nseconds(
                1_000_000_000 / u64::from(self.framerate),
            ));
        }
        if let Err(err) = appsrc.push_buffer(buffer) {
            self.push_fail.fetch_add(1, Ordering::Relaxed);
            debug!(error = %err, "capture push dropped");
        } else {
            self.pushed_frames.fetch_add(1, Ordering::Relaxed);
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
        dmabuf: Option<DmaBufImport>,
    ) -> Result<Self, String> {
        let (ready_tx, ready_rx) = std::sync::mpsc::channel();
        let join = std::thread::Builder::new()
            .name("xs-capture".into())
            .spawn(move || capture_thread(node_id, width, height, hub, dmabuf, ready_tx))
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
    dmabuf: Option<DmaBufImport>,
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

    if let Err(e) = run_capture_stream(&mainloop, node_id, width, height, hub, dmabuf) {
        warn!(error = %e, "screen-cast stream failed");
    }
}

fn run_capture_stream(
    mainloop: &pw::main_loop::MainLoopRc,
    node_id: u32,
    width: u32,
    height: u32,
    hub: Arc<CursorHub>,
    dmabuf: Option<DmaBufImport>,
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

    let dequeue_only = std::env::var("EXTRASPACE_PIPELINE")
        .map(|v| v == "dequeue")
        .unwrap_or(false);
    let allocator = dmabuf
        .as_ref()
        .map(|_| gstreamer_allocators::DmaBufAllocator::new());
    let data = CaptureData {
        hub,
        format: spa::param::video::VideoInfoRaw::new(),
        warned_unmapped: false,
        warned_format: false,
        dequeue_only,
        dmabuf: dmabuf.clone(),
        allocator,
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
            if let Some(update) = cursor {
                data.hub.store_cursor(update);
            }
            if data.dequeue_only {
                if unsafe { video_chunk_size(raw) } > 0 {
                    data.hub.mutter_frames.fetch_add(1, Ordering::Relaxed);
                }
            } else if let Some(import) = data.dmabuf.clone().filter(|_| unsafe { is_dmabuf(raw) }) {
                let stride = unsafe { video_chunk_stride(raw) };
                if let Some(buffer) = unsafe { wrap_dmabuf_frame(raw, data, &import) } {
                    data.hub.mutter_frames.fetch_add(1, Ordering::Relaxed);
                    let width = data.format.size().width;
                    let height = data.format.size().height;
                    data.hub
                        .on_dmabuf_frame(buffer, width, height, &import.drm_format(), stride);
                }
            } else {
                let copied = std::time::Instant::now();
                let video = unsafe { copy_video_frame(raw, data) };
                if video.is_some() {
                    data.hub
                        .observe_copy_us(copied.elapsed().as_micros() as u64);
                    data.hub.mutter_frames.fetch_add(1, Ordering::Relaxed);
                }
                if let Some(frame) = video {
                    data.hub.on_video_frame(frame);
                }
            }
            unsafe { stream.queue_raw_buffer(raw) };
        })
        .register()
        .map_err(|e| e.to_string())?;

    // The dma-buf format is offered first so mutter prefers it, with the
    // system-memory format left in place as the fallback.
    let dmabuf_bytes = dmabuf
        .as_ref()
        .map(|import| video_enum_format_dmabuf_pod(width, height, import));
    let format_bytes = video_enum_format_pod(width, height);
    let meta_bytes = cursor_meta_pod();
    let buffers_bytes = buffers_pod(dmabuf.is_some());
    let format_pod = spa::pod::Pod::from_bytes(&format_bytes).ok_or("video format pod")?;
    let meta_pod = spa::pod::Pod::from_bytes(&meta_bytes).ok_or("cursor meta pod")?;
    let buffers_pod = spa::pod::Pod::from_bytes(&buffers_bytes).ok_or("buffers pod")?;
    let dmabuf_pod = match dmabuf_bytes.as_ref() {
        Some(bytes) => Some(spa::pod::Pod::from_bytes(bytes).ok_or("dma-buf format pod")?),
        None => None,
    };
    let mut params: Vec<&spa::pod::Pod> = Vec::new();
    params.extend(dmabuf_pod);
    params.extend([format_pod, meta_pod, buffers_pod]);
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
            spa::utils::Rectangle { width, height },
            spa::utils::Rectangle { width, height }
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

fn buffers_pod(dmabuf: bool) -> Vec<u8> {
    use spa::pod::{Object, Property, Value};

    let mut types = (1 << spa::sys::SPA_DATA_MemPtr) | (1 << spa::sys::SPA_DATA_MemFd);
    if dmabuf {
        types |= 1 << spa::sys::SPA_DATA_DmaBuf;
    }
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

/// A dma-buf layout the GPU can import, as negotiated with mutter.
#[derive(Debug, Clone)]
pub struct DmaBufImport {
    /// SPA/GStreamer video format name, e.g. `BGRA`.
    pub spa_format: &'static str,
    /// The same layout as a DRM fourcc, e.g. `AR24`, for the GStreamer caps.
    pub fourcc: &'static str,
    /// DRM format modifier describing the tiling.
    pub modifier: u64,
}

impl DmaBufImport {
    fn video_format(&self) -> spa::param::video::VideoFormat {
        use spa::param::video::VideoFormat;
        match self.spa_format {
            "BGRA" => VideoFormat::BGRA,
            "RGBA" => VideoFormat::RGBA,
            "RGBx" => VideoFormat::RGBx,
            _ => VideoFormat::BGRx,
        }
    }

    fn gst_format(&self) -> gstreamer_video::VideoFormat {
        match self.spa_format {
            "BGRA" => gstreamer_video::VideoFormat::Bgra,
            "RGBA" => gstreamer_video::VideoFormat::Rgba,
            "RGBx" => gstreamer_video::VideoFormat::Rgbx,
            _ => gstreamer_video::VideoFormat::Bgrx,
        }
    }

    /// `drm-format` field value, e.g. `AR24:0x0100000000000002`.
    fn drm_format(&self) -> String {
        format!("{}:{:#018x}", self.fourcc, self.modifier)
    }
}

/// EnumFormat offering exactly one dma-buf layout.
///
/// A single modifier is deliberate: offering a choice means the producer may
/// hand back a choice of its own, which then has to be fixated in a second
/// round of negotiation. The GPU can import one layout for this format, so
/// there is nothing to choose between -- and if mutter cannot produce it, the
/// system-memory EnumFormat that follows this one is used instead.
fn video_enum_format_dmabuf_pod(width: u32, height: u32, import: &DmaBufImport) -> Vec<u8> {
    use spa::param::format::{FormatProperties, MediaSubtype, MediaType};
    use spa::pod::{Object, Property, PropertyFlags, Value};
    use spa::utils::Id;

    let obj = Object {
        type_: spa::utils::SpaTypes::ObjectParamFormat.as_raw(),
        id: spa::param::ParamType::EnumFormat.as_raw(),
        properties: vec![
            Property::new(
                FormatProperties::MediaType.as_raw(),
                Value::Id(Id(MediaType::Video.as_raw())),
            ),
            Property::new(
                FormatProperties::MediaSubtype.as_raw(),
                Value::Id(Id(MediaSubtype::Raw.as_raw())),
            ),
            Property::new(
                FormatProperties::VideoFormat.as_raw(),
                Value::Id(Id(import.video_format().as_raw())),
            ),
            // Mandatory: its presence is what tells the producer this client can
            // take dma-bufs at all.
            Property {
                key: FormatProperties::VideoModifier.as_raw(),
                flags: PropertyFlags::MANDATORY,
                value: Value::Long(import.modifier as i64),
            },
            Property::new(
                FormatProperties::VideoSize.as_raw(),
                Value::Rectangle(spa::utils::Rectangle { width, height }),
            ),
            Property::new(
                FormatProperties::VideoFramerate.as_raw(),
                Value::Fraction(spa::utils::Fraction { num: 0, denom: 1 }),
            ),
        ],
    };
    serialize_pod(Value::Object(obj))
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

/// Whether mutter filled this buffer with a dma-buf rather than mapped memory.
/// Both are offered during negotiation, so the answer can change per stream.
unsafe fn is_dmabuf(pw_buf: *mut pw_sys::pw_buffer) -> bool {
    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() || (*spa_buf).n_datas == 0 || (*spa_buf).datas.is_null() {
        return false;
    }
    (*(*spa_buf).datas).type_ == spa::sys::SPA_DATA_DmaBuf
}

/// Row pitch mutter allocated, which for a tiled buffer is padded past
/// `width * 4`.
unsafe fn video_chunk_stride(pw_buf: *mut pw_sys::pw_buffer) -> i32 {
    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() || (*spa_buf).n_datas == 0 || (*spa_buf).datas.is_null() {
        return 0;
    }
    let spa_data = (*spa_buf).datas;
    if (*spa_data).chunk.is_null() {
        return 0;
    }
    (*(*spa_data).chunk).stride
}

unsafe fn video_chunk_size(pw_buf: *mut pw_sys::pw_buffer) -> u32 {
    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() || (*spa_buf).n_datas == 0 || (*spa_buf).datas.is_null() {
        return 0;
    }
    let spa_data = (*spa_buf).datas;
    if (*spa_data).chunk.is_null() {
        return 0;
    }
    (*(*spa_data).chunk).size
}

/// Wraps a dma-buf frame as a GStreamer buffer without touching the pixels.
///
/// The fd is duplicated so the PipeWire buffer can be requeued immediately;
/// GStreamer closes its copy when the buffer is released, and the underlying
/// GEM object stays alive as long as either fd is open.
unsafe fn wrap_dmabuf_frame(
    pw_buf: *mut pw_sys::pw_buffer,
    data: &mut CaptureData,
    import: &DmaBufImport,
) -> Option<gst::Buffer> {
    use gstreamer_allocators::prelude::DmaBufAllocatorExtManual;

    let spa_buf = (*pw_buf).buffer;
    if spa_buf.is_null() || (*spa_buf).n_datas == 0 || (*spa_buf).datas.is_null() {
        return None;
    }
    let spa_data = (*spa_buf).datas;
    if (*spa_data).type_ != spa::sys::SPA_DATA_DmaBuf || (*spa_data).chunk.is_null() {
        return None;
    }
    let chunk = *(*spa_data).chunk;
    if chunk.size == 0 {
        return None;
    }
    let fd = (*spa_data).fd as std::os::fd::RawFd;
    if fd < 0 {
        return None;
    }
    let allocator = data.allocator.as_ref()?;
    let owned = match unsafe { std::os::fd::BorrowedFd::borrow_raw(fd) }.try_clone_to_owned() {
        Ok(owned) => owned,
        Err(e) => {
            warn!(error = %e, "could not duplicate the dma-buf fd for this frame");
            return None;
        }
    };
    let size = (*spa_data).maxsize as usize;
    let memory = match unsafe { allocator.alloc_dmabuf(owned, size) } {
        Ok(memory) => memory,
        Err(e) => {
            warn!(error = %e, "could not wrap the dma-buf as GStreamer memory");
            return None;
        }
    };
    let mut buffer = gst::Buffer::new();
    let width = data.format.size().width;
    let height = data.format.size().height;
    {
        let buffer = buffer.get_mut()?;
        buffer.append_memory(memory);
        // The row pitch is whatever the GPU allocated, which for a width that is
        // not a multiple of the tile size is wider than `width * 4`. Without the
        // real value here every row is read at the wrong offset and the image
        // shears diagonally, so it is carried explicitly rather than inferred.
        let stride = if chunk.stride > 0 {
            chunk.stride
        } else {
            (width * 4) as i32
        };
        if let Err(e) = gstreamer_video::VideoMeta::add_full(
            buffer,
            gstreamer_video::VideoFrameFlags::empty(),
            import.gst_format(),
            width,
            height,
            &[chunk.offset as usize],
            &[stride],
        ) {
            warn!(error = %e, "could not describe the dma-buf layout");
            return None;
        }
    }
    Some(buffer)
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
