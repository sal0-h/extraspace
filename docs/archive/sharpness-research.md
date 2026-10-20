# Sharpness research

Researched October 19, 2026, against application commit `c732a3d`, before the
native Wi-Fi trials and recovery fixes. The [plan](sharpness-plan.md) records the
experiment order; the [results](../benchmarks/sharpness.md) describe the shipped
H.264 solution. The alternatives below remain unproven on this device pair.

## Baseline

The earlier [USB comparison](resolution-benchmark.md) showed that native
2304 × 1440 could sustain 59.7 decoded FPS in GPU text scrolling, compared with
59.9 FPS at 1536 × 960. CPU/Cairo scrolling fell from 54.6 to 30.7 FPS. Median
host pipeline time rose from 4.9 to 8.9 ms in GPU scrolling. These were short
synthetic USB runs, not full input-to-display measurements.

Native resolution has 2.25 times as many pixels. Compression can reduce network
bytes, but drawing, conversion, decoding, and presentation still process those
pixels. A transport change cannot recover frames the source application did
not draw.

The baseline already used DMA-BUF import, GPU conversion/encoding, no B-frames,
shallow queues, Android low-latency requests, damage-driven capture, and separate
cursor updates. The initial investigation therefore focused on these options.

## H.264 tuning

The [encoder](../../crates/xs-video/src/encoder.rs) uses a maximum keyframe
interval of 24 frames, about 0.4 seconds during steady 60 FPS motion, plus an
explicit keyframe request on connection. Longer intervals might reduce refresh
bursts or improve quality at a fixed bitrate. They do not necessarily reduce
average CBR traffic. Intel's low-delay configurations use long GOPs without
B-frames, but include other reference/buffer policies that need separate
validation. [Intel low-delay guidance](https://github.com/intel/media-delivery/blob/master/doc/quality-lowdelay.rst)

The baseline could discard encoded reference frames under pressure. Recovery
had to be fixed before testing longer intervals. The shipped implementation now
preserves encoded references and drops stale raw frames before encoding.

The laptop's VA encoder exposes `target-usage` values 1–7, defaulting to 4.
Compare speed settings at matched text quality. Starting bitrate also depends
on pixel count, so native output's higher bitrate is partly policy rather than
proof of a minimum required rate.
[GStreamer H.264 controls](https://gstreamer.freedesktop.org/documentation/va/vah264lpenc.html)

## Hardware H.265

The Raptor Lake-P capability probe found `vah264lpenc` and `vah265lpenc` with
Intel's iHD driver 26.3.5. `vainfo` advertised H.264 and HEVC low-power encoding;
no `vaav1enc` element was available. This was a capability probe, not a codec
performance comparison.

H.265 may improve quality per bit, but its encode/decode time and power cost
must be measured on the actual devices. Compare equal-bitrate quality, then
bandwidth and timing at comparable text quality.
[Intel HEVC configuration](https://github.com/intel/media-delivery/blob/master/doc/quality-lowdelay.rst#h265hevc),
[GStreamer HEVC encoder](https://gstreamer.freedesktop.org/documentation/va/vah265lpenc.html)

Before implementation, query the tablet's hardware decoder, supported sizes
and rates, low-latency feature, and performance points when provided. A request
for low latency does not establish support. Later trials confirmed hardware
H.264 low-latency support; they did not validate HEVC throughput.
[Android codec capabilities](https://developer.android.com/reference/android/media/MediaCodecInfo.CodecCapabilities#FEATURE_LowLatency),
[Android performance points](https://developer.android.com/reference/android/media/MediaCodecInfo.VideoCapabilities#getSupportedPerformancePoints())

The protocol has a codec field, but the host emits H.264 and Android assumes AVC.
H.265 needs capability negotiation and an H.264 fallback.

## Android presentation

The app uses `TextureView`. Android recommends `SurfaceView` where suitable for
frame timing and power. Earlier project work recorded a Samsung cursor-overlay
issue with `SurfaceView`; any experiment must preserve overlay visibility and
touch mapping. No replacement was justified by these measurements.
[Android surface guidance](https://developer.android.com/media/media3/ui/surface)

## Colored text fidelity

The GPU path converts RGB to NV12 with 4:2:0 chroma sampling. Fine colored text
can lose detail even at native luma resolution. Microsoft's research and remote
desktop guidance describe full-resolution color approaches.
[Full-resolution color research](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/DCC1320Wu_et_al20Hi-Res20Color.pdf),
[4:4:4 guidance](https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-chroma-value-increase-4-4-4)

The host driver advertises HEVC 4:4:4 and screen-content profiles, which does not
establish tablet compatibility. Changed RGB tiles or a separate static-detail
layer could preserve color detail, at the cost of more protocol/compositing
work and potentially more data. Microsoft's mixed-mode implementation is a
design reference, not a ready-made hardware path for Extraspace.
[Graphics encoding](https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding)

## Video age and transport

Control, video, and camera already use separate TLS/TCP connections with
TCP_NODELAY. A healthy control ping does not establish that video is current,
and shallow application queues do not bound kernel, TLS, or decoder queues.
Measure video write stalls and receiver feedback before changing transport.
The sharpness work added gap/write diagnostics; end-to-end frame-age feedback
remains incomplete.

TCP retransmits missing bytes in order, so older video can delay newer data on
that connection. QUIC DATAGRAM permits encrypted delivery without mandatory
retransmission. It could reduce stale delivery under loss but cannot increase
Wi-Fi capacity or accelerate drawing. A reliable QUIC stream still orders its
own bytes. [TCP specification](https://www.rfc-editor.org/rfc/rfc9293.html#section-2.2),
[QUIC DATAGRAM specification](https://www.rfc-editor.org/rfc/rfc9221.html)

A datagram experiment needs MTU-aware packetization, reassembly, frame
dependencies, refresh requests, feedback, congestion control, authentication,
and a fallback for networks that block UDP. Prioritize it only if video-age and
loss measurements identify a transport problem.
