# Engineering notes

These observations explain capture and connection decisions in the current
implementation. Measurements belong to the hardware and workload described;
see the [latest benchmark](benchmarks/sharpness.md) for native-resolution Wi-Fi.

## Buffer lifetime and layout

A duplicated DMA-BUF file descriptor keeps an allocation alive, but returning
the PipeWire buffer allows the producer to overwrite its pixels. Hold the
capture buffer until the last imported GStreamer memory reference is released,
then requeue it on the owning PipeWire loop. Track generations to reject returns
for removed or replaced buffers.

GPU row pitch can exceed `width * 4`. Observed strides include 9216 bytes for
2296 pixels and 5376 bytes for 1316 pixels. Attach the reported stride through
`GstVideoMeta`; guessing from caps shifts rows and shears the image. Test
unaligned widths as well as convenient sizes such as 1920.

A slow consumer can throttle capture because Mutter needs its buffers back.
One earlier 1920 × 1080 animated workload produced 14.5 FPS with the software
chain and 39.4 FPS with the GPU chain.

## Pixel movement and encoding

On an i7-1355U, an earlier 2296 × 1428 software pipeline spent 22.4 ms per frame
in conversion and x264. More x264 threads or an `ultrafast` preset recovered
little time. VA-API encoding took about 1 ms, but replacing only the encoder
left the CPU-to-GPU upload cost. DMA-BUF import removed the repeated CPU pixel
passes. Application drawing can still limit native-resolution performance;
see the [CPU/Cairo comparison](archive/resolution-benchmark.md).

`x264enc` accepts kbit/s; `openh264enc` accepts bit/s. Keep the conversion in
`xs-video::encoder`. An earlier `vulkanh264enc` test produced byte-identical
output at 5, 15, and 40 Mbit/s despite accepting the bitrate property, so the app
does not select it. The tested Fedora packages also lacked `nvh264enc`; GPU
vendor support must be checked on the actual installation.

A USB 2.0 link cannot carry raw 2000 × 1200 video at 60 FPS, but a 15 Mbps H.264
stream uses under 2 MB/s. Encoded bandwidth and raw capture bandwidth are
different constraints.

## Frame pacing and decoding

Mutter captures on damage. A still desktop can produce about 11 FPS or fewer;
60 FPS is a ceiling, not a promised idle rate. Default `videorate` duplicates
frames to fill gaps and can accumulate latency, so pacing is drop-only.

Drop stale raw frames before encoding. Dropping an encoded P-frame can leave
subsequent frames referring to missing data, with corruption lasting until a
keyframe. The encoded channel applies backpressure instead. Android waits for
codec input capacity and reconnects after a persistent stall.

Drain MediaCodec output independently of new input. When draining happened only
on input arrival, the last frame remained decoded but unrendered on an idle
desktop. That also distorted queue telemetry and kept adaptive bitrate low.
Wait for decoder startup before sending the first access unit so SPS/PPS and
the initial keyframe are retained.

Encoder keyframe intervals count frames. An interval of `framerate * 2` can
mean a long wall-clock wait while idle. The current maximum is 24 frames, with
an explicit keyframe request when a tablet attaches.

## Cursor rendering

Mutter's embedded cursor updates with desktop damage. Pointer motion alone can
leave it frozen over a still window. Capture `SPA_META_Cursor` through the same
PipeWire consumer as video and send bitmap/position updates to Android's overlay.

Earlier attempts using a second PipeWire consumer hit unfixed caps in
`gst-plugin-pipewire`. A fullscreen GTK damage window obscured the virtual
monitor. Neither approach is used now.

## ADB sockets

`adb forward` can accept a local connection even when the device has no listener,
then close it after the remote open fails. Treat handshake bytes as readiness;
connection success alone is insufficient.

Dropping Tokio's `OwnedWriteHalf` calls `shutdown(Write)`. ADB can then tear down
the whole forwarded socket, causing the peer's next write to fail with EPIPE.
Keep unused halves alive, or avoid splitting one-direction streams.

## Original upstream results

Upstream tested GNOME Shell 50.3 with a Telekom T Tablet (Wingtech, Android 15)
over USB. It recorded 55 FPS at 1332 × 800 with x264, a first frame 47 ms after
start, roughly 1 ms control round trip, and a decoder input queue of zero.
Virtual displays, touch, APK installation, adaptive bitrate, and 1920 × 1080
camera export through `/dev/video10` worked on that setup. These are historical
measurements, not current latency or compatibility guarantees.
