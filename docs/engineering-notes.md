# Engineering notes

These notes preserve measurements and debugging results from the original
project and later work in this fork. They describe the machines and software
used at the time; they are not current benchmarks for every supported setup.

The [October 2026 resolution comparison](resolution-benchmark.md) measures this
fork on Salman's current tablet and explains why the smaller 1.5× stream remains
the default.

## Original hardware results

Everything below has been run end to end on real hardware — GNOME Shell 50.3
driving a Telekom T Tablet (Wingtech, Android 15) over USB.

| Piece | State |
|---|---|
| Virtual monitor creation, teardown | **Working** — appears alongside physical outputs |
| Capture → H.264 encode | **Working** — 55 fps at 1332×800 via `x264enc` |
| USB transport, handshake, APK auto-push | **Working** |
| Tablet-side decode and display | **Working** — MediaCodec, `low-latency=true` |
| Touch → cursor on the virtual monitor | **Working** — coordinates map exactly |
| Adaptive bitrate control | **Working** — queue 0, RTT ~1 ms, probes upward |
| Camera → `/dev/video10` | **Working** — 1920×1080 in any V4L2 app |
| Keyboard, stylus, audio, Wi-Fi | Not started — see [Roadmap](#roadmap) |

Measured on that setup: first frame **47 ms** after start, round trip **~1 ms**
over USB, decoder input queue steady at **0**.

This is a v0.1 that has been made to work on exactly one tablet and one GNOME
version. Reports from other hardware are the most useful thing you can send.

## Capture, encoding, and transport

Things that cost time, recorded so they cost you less:

- **`vulkanh264enc` silently ignores its bitrate setting.** It advertises CBR and
  accepts the property, but produces byte-identical output at 5, 15 and 40 Mbit/s.
  Unusable for adaptive streaming. Extraspace does not offer it.
- **Fedora strips NVENC out of GStreamer's `nvcodec` plugin.** Only the CUDA
  utility elements register; `nvh264enc` does not exist, and `plugins-freeworld`
  does not add it. `x264enc` at `veryfast` manages ~168 fps at 2000×1200 on a
  mid-range CPU, which is 2.8× more than needed, so this matters less than it sounds.
- **`x264enc` takes kbit/s but `openh264enc` takes bit/s.** A 1000× error waiting
  to happen; the conversion lives in exactly one function.
- **At panel resolution the cost is moving pixels, not encoding them.** A
  2296×1428 frame is 13 MB, and the software chain moved it through DRAM four
  times: mutter's readback, our copy, `videoconvert`, then x264. Measured on an
  i7-1355U, `videoconvert` plus x264 cost 22.4 ms per frame — a 45 fps ceiling
  before mutter did any work — and going from 2 to 6 x264 threads, or down to
  `ultrafast`, recovered barely a tenth of it. VA-API encode of the same frames
  costs about 1 ms, but swapping *only* the encoder gains nothing, because
  uploading the frame costs what the conversion did. Taking dma-bufs from mutter
  removes all four passes at once.
- **A dma-buf's row pitch is not `width * 4`.** The GPU pads each row out to a
  tile boundary: 2296 pixels are allocated as 9216 bytes rather than 9184, and
  1316 as 5376 rather than 5264. Letting GStreamer infer the pitch from the caps
  shifts every row and shears the image diagonally, so the pitch mutter reports
  is attached to each buffer as a `GstVideoMeta`. Widths that happen to be
  aligned — 1920 is exactly 15 tiles — look perfect either way, which makes this
  a bug that only appears at the resolutions that matter. Verify at those.
- **A slow consumer throttles the compositor.** Mutter will not produce a new
  frame until the previous buffer comes back, so encoding speed sets the capture
  rate rather than merely following it. Under one animated load at 1920×1080,
  mutter delivered 14.5 fps to the software chain and 39.4 fps to the GPU one.
- **USB 2.0 is not the bottleneck.** Raw 2000×1200@60 would need ~550 MB/s, far
  beyond the ~30 MB/s a High Speed link gives you. Encoded H.264 at 15 Mbit/s is
  under 2 MB/s — roughly 15× headroom.
- **`adb forward` accepts connections to nothing.** adb accepts the *local* TCP
  connection whether or not anything is listening on the device, then closes it
  once the remote open fails. So `connect()` succeeds on the first attempt even
  when the companion app has not started, and the failure appears milliseconds
  later as an unexplained EOF mid-handshake. Retrying on connection-refused never
  helps, because connection-refused never happens. The only reliable readiness
  signal is bytes.
- **Half-closing one direction kills the whole channel.** Tokio's
  `OwnedWriteHalf` calls `shutdown(Write)` when dropped, and adb's forwarder tears
  down the entire unix socket to the device when either direction closes. Splitting
  a read-only stream and dropping the unused write half is therefore fatal — the
  peer's next write gets EPIPE. Streams used one way are not split at all here.
- **Mutter's embedded cursor only updates when the desktop is damaged.** Pointer
  motion is a hardware plane, not damage, so a still window freezes the cursor.
  Metadata mode plus a single PipeWire consumer that blits `SPA_META_Cursor` is
  the fix. A second consumer (`pipewiresrc` plus a listener) makes
  gst-plugin-pipewire abort on unfixed caps. A fullscreen GTK "damage pump"
  becomes an opaque black window on Meta-0.
- **Default `videorate` invents frames.** On a damage-driven capture (idle ~11
  fps) it duplicates the last buffer to fill 60 fps holes and builds seconds of
  fake catch-up latency. The pipeline is drop-only.
- **Draining MediaCodec only when input arrives loses the last frame.** Because
  mutter sends only on damage, an idle desktop delivers nothing for seconds at a
  time; if output is drained inside the input path, the final frame stays decoded
  but unrendered until something else changes. A dedicated drain thread fixed both
  that and the queue-depth telemetry, which had been reporting a stuck 6–8 and
  driving the bitrate to the floor for entire sessions.
- **Drop stale raw frames before encoding.** An encoded P-frame can be a reference
  for the next frame. Discarding it from a full output queue caused blocks to
  remain on a static native-resolution page until another keyframe arrived.
  The encoded channel now preserves references and propagates backpressure to
  the one-frame leaky raw queue. Decoder input saturation also waits briefly,
  then reconnects if necessary, rather than silently losing a reference.
- **A duplicated DMA-BUF fd does not preserve the frame's contents.** It keeps
  the allocation alive, while returning the PipeWire buffer permits the producer
  to overwrite it. Imported GStreamer memory now retains the capture buffer
  until its last reference is released, then asks the owning loop to recycle it.
- **"60 fps" is a ceiling, not a rate.** Mutter only emits a frame when something
  on the monitor actually changes, so a still desktop measures around **11 fps**
  and well under 1 Mbit/s. That is exactly what you want — an idle screen should
  be nearly free — but it quietly breaks anything that reasons in frame counts.
  Both encoders express their keyframe interval in frames, so the obvious
  `framerate × 2` puts keyframes *ten seconds* apart while idle, and a tablet
  that reconnects sits on a black screen until one arrives. The interval is sized
  against the idle rate instead, and a keyframe is requested explicitly whenever a
  tablet attaches.
