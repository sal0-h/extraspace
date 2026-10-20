# Native-resolution Wi-Fi benchmark

Measured October 19–20, 2026, with a Samsung SM-X510 (2304 × 1440, Android 16),
an Intel Raptor Lake-P laptop, GNOME/Mutter 51.0-2 with the local patches, and
native paired Wi-Fi. USB was attached for logs; the benchmark explicitly used
Wi-Fi.

## Outcome

Sharper streams 2304 × 1440 at 1.5× while retaining a 1536 × 960 logical
workspace. Standard sends 1536 × 960 and enlarges it on the tablet. The GPU
scrolling fixture sustained about 60 decoded FPS in both modes. Native output
added about 3 ms to the measured host pipeline and used more bandwidth.

The installed launcher and Quick Settings workflow were checked: windowless
startup, Standard/Sharper switching over USB, settings open/close without stream
restart, and saved Wi-Fi reconnection after unplugging USB. The user confirmed
sharper text with comparable responsiveness. Sharper is now preferred when the
compositor check passes; Standard remains available.

## Measurements

Six 110-second trials used a GTK/GSK Vulkan fixture with 12 seconds
of static text, 64 of scrolling, 16 of motion, and 8 of static recovery. It
included small text, colored code, thin lines, and gray ramps. Camera forwarding
was off. Three runs used each resolution in Standard/native/native/Standard/
Standard/native order.

The initial comparison includes the encoded-reference fix below but predates
the PipeWire ownership fix. The final native run includes both fixes and an
intentional 500 ms sender pause near the start of scrolling. Steady analysis
excludes three seconds at each end of scrolling; the injected pause falls
before that window.

| Metric | Standard, mean of 3 | Native, initial mean of 3 | Native, final run |
|---|---:|---:|---:|
| Decoded FPS, steady scrolling | 59.84 | 59.87 | 59.75 |
| Host pipeline median | 4.71 ms | 7.92 ms | 7.85 ms |
| Host pipeline p95 | 6.03 ms | 8.84 ms | 8.51 ms |
| Encoded bandwidth | 25.42 Mbps | 47.42 Mbps | 35.61 Mbps |
| Streaming-process CPU, percent of one core | 12.37% | 15.16% | 14.23% |

The final run's lower bitrate reflects adaptive backoff after the injected
stall. H.264, the 24-frame keyframe interval, and the 6–50 Mbps bounds were
unchanged. Hardware video-engine busy time roughly doubled in the initial
comparison; power consumption was not measured.

Wi-Fi timing varied in both modes. Across full scrolling intervals, Standard
averaged 59.83–59.88 FPS and initial native runs 59.49–59.89 FPS. One Standard
run had a 273 ms receive gap. The last initial native run had a 210 ms video
write stall, raw-frame drops, and bitrate adaptation near the end of scrolling.
The trimmed steady window excludes that native incident.

Host pipeline timing uses GStreamer's appsrc-to-appsink latency tracer. It
excludes drawing, transmission, decoding, and panel presentation. CPU figures
exclude GNOME and the fixture. Decoded-frame counters count buffers released
to a surface, not confirmed panel presentations. No glass-to-glass latency or
battery-life measurement was made.

These results cover one device pair and a synthetic GPU workload. The earlier
[USB comparison](../archive/resolution-benchmark.md) found a CPU/Cairo workload
near 31 FPS at native resolution before transmission. Standard remains useful
when drawing or network capacity limits a workload.

## Recovery fixes

An initial native trial dropped an encoded reference frame and left blocks on
a static page until another keyframe arrived. The encoded queue now applies
backpressure, dropping stale raw frames before encoding. Android also waits
briefly for input capacity and reconnects after persistent saturation.

A later stall still produced a garbled recovery image. Review found that
capture returned PipeWire buffers while GStreamer could still be reading them.
Duplicating the DMA-BUF fd did not preserve the frame contents. Capture now
holds each buffer until the last imported memory reference is released and
requeues it on the owning loop, with generation checks. See the
[PipeWire buffer lifecycle](https://docs.pipewire.org/group__pw__stream.html).

After this fix, recovery captures were clean under concurrent build load and
two confirmed late 500 ms sender pauses. The final full-length run recovered
from a 500 ms pause after dropping 29 raw frames, with a clean static page and
an empty decoder input queue. Independent decoding of a complete recorded
stream from a repaired short trial also showed clean recovery. The original
late corruption was not recorded as a complete bitstream, so its first point
of corruption cannot be established from that image alone.

Regression tests cover shared-memory lifetime, raw-frame backpressure, blocked
encoder shutdown, explicit Wi-Fi selection, and saved quality controls.
Android identified `c2.exynos.h264.decoder` as hardware-accelerated with
low-latency support.

## Reproduction

Close the desktop host and leave the tablet app open:

```bash
cargo build --release --locked -p xs-core --example resolution_bench
target/release/examples/resolution_bench --resolution native --transport wifi --seconds 110
```

Once streaming, run `GSK_RENDERER=vulkan python3 scripts/sharpness-fixture.py`
from the repository root. The fixture logs phases and actual snapshot counts;
send `mark NAME` on benchmark stdin to align phases. `--quick` runs a short
recovery check rather than the full performance trial.

| Benchmark option | Purpose |
|---|---|
| `--resolution current` | Standard baseline; `native` selects Sharper |
| `--transport auto\|wifi\|usb` | Select transport; `wifi` bypasses ADB |
| `--serial SERIAL` | Pin a USB device; requires `--transport usb` |
| `--fixed-kbps N` | Fix bitrate for a matched-rate comparison |
| `--label NAME` | Label measurements |

The benchmark disables camera forwarding and does not rewrite saved app
settings. Timing begins at Streaming. stdin accepts `mark NAME`,
`snapshot NEW_DIRECTORY`, and `quit`; EOF does not end the run.

`EXTRASPACE_VIDEO_TRACE=NEW_FILE` records access units to a new private file.
`EXTRASPACE_VIDEO_TEST_STALL_MS` (up to 2000) and
`EXTRASPACE_VIDEO_TEST_STALL_FRAME` (default 120) inject one sender pause. Leave
these unset for normal use. Recordings contain desktop pixels.

Logs and captures were kept locally under `/tmp/extraspace-sharpness-*`,
including the `20261019-233416` comparison and `20261020-000149` final run.
They are temporary artifacts, not committed benchmark data. The fixture and
measurement code are versioned. Codec and transport experiments remain in the
[historical research](../archive/sharpness-research.md).
