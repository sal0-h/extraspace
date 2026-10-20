# Sharper text and responsiveness

Research dated October 19, 2026, against application commit `c732a3d`.
The target is sharper text with an interactive feel close to the current mode.
The candidates below have not been demonstrated to improve end-to-end latency
on the tablet. This research does not change the installed app's defaults.

The user has deferred direct wireless and stylus support. Sleep/wake and roaming
are unverified integration scenarios, not established failures. Keyboard
passthrough and remote use over a VPN are outside the current priorities. Palm
rejection would matter only if stylus input becomes a desired workflow.

The existing [resolution comparison](resolution-benchmark.md) gives the most
useful starting point:

| Workload | 1536 × 960 decoded FPS | 2304 × 1440 decoded FPS |
|---|---:|---:|
| GPU-rendered text scrolling | 59.9 | 59.7 |
| CPU/Cairo text scrolling | 54.6 | 30.7 |

For GPU scrolling, median host pipeline time increased from 4.9 to 8.9 ms.
Network round trips were about 2–3 ms in both modes, and the recorded analysis
found the Cairo slowdown in the workload's own drawing rate. Thus native
resolution already sustained approximately 60 FPS in one workload. A protocol
change cannot recover frames an application never draws. The comparison used
USB, short synthetic runs, and one tablet; native-resolution performance over
the current Wi-Fi transport and in the user's usual apps remains unmeasured.
Host pipeline time and control round trips are not full input-to-display delay.

Native resolution has 2.25 times as many pixels. Compression can reduce network
bytes, but drawing, conversion, decoding, and presentation still process those
pixels. A more efficient stream could reduce the penalty; eliminating it for
every application is not established.

The production path already imports DMA-BUF frames into VA surfaces, performs
conversion on the GPU, disables B-frames, uses a shallow queue, requests Android
low-latency decoding, and sends cursor updates separately. Capture is driven by
desktop changes, and H.264 already predicts changes between frames. Simply
adding GPU encoding, enabling a low-latency flag, or proposing that only changes
be transmitted would repeat work already present.

There are several concrete opportunities to investigate.

1. **Tune the existing H.264 path before adding another codec.**

   [The encoder](../crates/xs-video/src/encoder.rs) sets a maximum keyframe
   interval of 24 frames at a nominal 60 FPS. This is about 0.4 seconds during
   sustained 60 FPS motion. That choice was made to avoid long idle/reconnect
   waits. The session also explicitly requests a keyframe on connection.
   Longer intervals could reduce refresh bursts or spend a fixed bitrate more
   effectively. This is a hypothesis, not a measured saving; under CBR, better
   efficiency need not reduce average network traffic.

   Intel's low-delay guidance uses long GOPs and disables B-frames. Its reference
   configurations are useful starting points, not settings to copy unchanged
   into this app: they also use different reference-frame and buffer policies.
   [Intel low-delay encoding guidance](https://github.com/intel/media-delivery/blob/master/doc/quality-lowdelay.rst)

   Before lengthening the GOP, ensure that dropped reference frames trigger
   recovery. The current encoded-frame queue can discard frames when full;
   subsequent predicted frames may depend on them. Startup, overload, and
   decoder restart must recover promptly. A longer GOP without that handling
   could prolong corruption. Longer GOPs do not require B-frames or lookahead.

   The laptop's encoder exposes `target-usage` values 1–7 and currently defaults
   to 4. GStreamer documents this as a speed/quality control. Compare the current
   setting with a faster setting at matched text quality. The app also chooses
   its starting bitrate from pixel count, so the higher bitrate in the native
   benchmark is partly policy, not proof that the content requires that rate.
   [GStreamer encoder controls](https://gstreamer.freedesktop.org/documentation/va/vah264lpenc.html)

2. **Compare hardware HEVC/H.265 with H.264.**

   A capability probe on the actual Raptor Lake-P laptop found both
   `vah264lpenc` and `vah265lpenc`, using Intel's iHD driver 26.3.5.
   `vainfo` reported H.264 and HEVC low-power encode entry points. No
   `vaav1enc` element was available, so AV1 encoding is not a first experiment
   on this setup. The probe queried capabilities; it was not a performance run.

   HEVC is a candidate for better image quality per network bit. Intel provides
   hardware low-delay configurations for both codecs, and GStreamer provides a
   VA-memory HEVC path with configurable B-frames. Compression efficiency does
   not guarantee lower encode/decode latency or power consumption on this pair
   of devices. Compare actual text, frame sizes, frame timing, and power.
   [Intel low-delay HEVC configuration](https://github.com/intel/media-delivery/blob/master/doc/quality-lowdelay.rst#h265hevc),
   [GStreamer HEVC encoder](https://gstreamer.freedesktop.org/documentation/va/vah265lpenc.html)

   The tablet was not connected over ADB during this research. Its exact HEVC
   decoder, native-resolution throughput, and low-latency support have not been
   verified. Collect codec names, hardware acceleration, supported sizes/rates,
   performance points, and `FEATURE_LowLatency` first. Android exposes these
   capabilities, although vendors may omit performance-point information.
   The current app selects a decoder by AVC MIME type and requests low latency;
   that request alone does not prove the chosen decoder supports the feature.
   [Android codec capabilities](https://developer.android.com/reference/android/media/MediaCodecInfo.CodecCapabilities#FEATURE_LowLatency),
   [Android performance points](https://developer.android.com/reference/android/media/MediaCodecInfo.VideoCapabilities#getSupportedPerformancePoints())

   The protocol already contains a codec field, but the host emits H.264 and
   the Android connection/decoder path assumes AVC. HEVC requires capability
   negotiation and an H.264 fallback, not just changing the host encoder name.

3. **Measure the Android presentation path.**

   The app renders through `TextureView`. Android recommends `SurfaceView`
   where suitable, citing better frame timing and lower power consumption on
   many devices. This is a candidate for reducing presentation overhead rather
   than network bytes. The repo records an earlier Samsung cursor-overlay issue
   with `SurfaceView`; a test must preserve cursor visibility and touch mapping.
   This should be an isolated experiment, not an unconditional replacement.
   [Android surface guidance](https://developer.android.com/media/media3/ui/surface)

4. **Consider text-aware color transport if colored text remains soft.**

   The current GPU path converts RGB to NV12, which uses 4:2:0 chroma sampling.
   Native luma resolution and full color resolution are different questions.
   Fine colored text can lose detail through chroma subsampling even when the
   framebuffer is native-sized. Microsoft's research demonstrates the issue
   for sharp graphics and colored text; Azure Virtual Desktop also exposes a
   4:4:4 fidelity option.
   [Microsoft research on full-resolution color](https://www.microsoft.com/en-us/research/wp-content/uploads/2016/02/DCC1320Wu_et_al20Hi-Res20Color.pdf),
   [Microsoft 4:4:4 guidance](https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-chroma-value-increase-4-4-4)

   The host driver advertises HEVC 4:4:4 and screen-content encode profiles, but
   that does not establish tablet decoder compatibility. Standard HEVC Main
   support would not establish support for those additional profiles either.
   Lossless changed RGB tiles or a separate color-detail layer are other
   possible designs. They add protocol/compositing work and can increase bytes;
   neither removes the upstream cost of drawing at native resolution.

   Mixed text/image/video encoding and progressive refinement are established
   remote-display techniques. Microsoft's implementation uses content-specific
   codecs in its software mixed mode; this is evidence for the design, not a
   ready-made hardware path for this app. A sharp static-text refinement layer
   could preserve detail without requiring every moving frame to be lossless.
   It would need new Android compositing and correct invalidation when content
   changes. This is a larger experiment than tuning the existing encoder.
   [Microsoft graphics encoding](https://learn.microsoft.com/en-us/azure/virtual-desktop/graphics-encoding)

5. **Control video age, and change transport only if loss is the bottleneck.**

   Wi-Fi already uses separate TLS/TCP connections for control, video, and
   camera, with TCP_NODELAY. Cursor/control traffic therefore does not share the
   video's ordered byte stream. The shallow application queue also does not
   prove that kernel, TLS, network, and decoder queues are short.

   The current adaptive controller considers decoder input depth, frame drops,
   and control-channel RTT. Its RTT congestion threshold is 120 ms and it waits
   for two bad samples before reducing bitrate. A healthy control ping does not
   establish that video is fresh on a separate connection. First measure video
   send stalls and capture-to-presentation age, then evaluate earlier feedback
   using those signals. Compare visual quality at a fixed latency budget rather
   than lowering resolution by default. A TCP write completing is not proof
   that the tablet received or presented a frame.

   TCP supplies reliable, ordered bytes, recovering losses through
   retransmission. This means missing older video bytes can delay newer bytes
   on that video connection. QUIC DATAGRAM offers encrypted, congestion-
   controlled delivery without mandatory retransmission, with optional
   expiration support in implementations. It is a candidate for reducing stale
   video delivery under loss, not a way to create more Wi-Fi capacity or speed
   up application drawing. A single reliable QUIC stream would still order its
   own bytes.
   [TCP specification](https://www.rfc-editor.org/rfc/rfc9293.html#section-2.2),
   [QUIC DATAGRAM specification](https://www.rfc-editor.org/rfc/rfc9221.html)

   A datagram experiment needs packetization/reassembly within path MTU,
   dependency-aware frame dropping, refresh requests, receiver feedback, and
   congestion control. Keep trusted-device authentication and a TCP fallback
   for networks that block UDP. Prioritize this only if frame-age and loss
   measurements show transport stalls; current USB measurements cannot justify
   a wireless transport rewrite.

The first experiment should compare current and native H.264 in real reading,
coding, and scrolling workloads, isolating application drawing, capture,
conversion/encode, transport, decode, and presentation. Then compare keyframe
policy and encoder speed at matched visual quality; test hardware HEVC if
network transmission or compression quality remains limiting. A USB baseline
helps distinguish network effects from rendering and coding cost.

Record FPS and frame gaps under motion, per-stage median/p95 timings, encoded
frame bursts, reconnect recovery, text captures including colored text, and
device load. Include video queue age and retransmissions in Wi-Fi runs. Keep
clock domains explicit. Establish actual input-to-display
delay before claiming that a lower network round trip improved responsiveness.
Use a Sharper profile as a fallback if the measured cost remains meaningful;
it need not be accepted as a permanent limitation before these experiments.
