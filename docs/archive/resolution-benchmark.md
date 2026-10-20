# USB resolution comparison

Measured October 14, 2026, before the native Wi-Fi sharpness work. At the time,
Standard at 1536 × 960 remained the default because the CPU/Cairo workload slowed
at native resolution. The later [Wi-Fi benchmark](../benchmarks/sharpness.md)
led to Sharper becoming preferred on compatible compositors and added recovery
fixes. This report preserves the earlier measurements.

Both modes provide a 1536 × 960 desktop workspace. Standard enlarges its
video on the tablet; native mode uses GNOME fractional scaling and sends the
panel's full pixel resolution.

## Results

Each row averages two runs. FPS comes from the tablet's decoded-frame counter,
not from the frequency of health reports. Pipeline columns average the per-run
median and 95th percentile, respectively.

| Workload | Stream | Decoded fps | Host pipeline median | Host pipeline p95 | Encoded bandwidth |
| --- | --- | ---: | ---: | ---: | ---: |
| Cairo text scrolling | 1536 × 960 | 54.6 | 5.3 ms | 7.3 ms | 18.1 Mbps |
| Cairo text scrolling | 2304 × 1440 | 30.7 | 9.6 ms | 14.8 ms | 21.9 Mbps |
| Cairo full-screen motion | 1536 × 960 | 53.7 | 5.1 ms | 6.1 ms | 18.2 Mbps |
| Cairo full-screen motion | 2304 × 1440 | 30.1 | 10.1 ms | 13.7 ms | 17.6 Mbps |
| GPU text scrolling | 1536 × 960 | 59.9 | 4.9 ms | 6.3 ms | 18.1 Mbps |
| GPU text scrolling | 2304 × 1440 | 59.7 | 8.9 ms | 10.0 ms | 38.3 Mbps |
| GPU color-node motion | 1536 × 960 | 31.3 | 5.2 ms | 6.6 ms | 11.4 Mbps |
| GPU color-node motion | 2304 × 1440 | 31.0 | 9.4 ms | 11.4 ms | 21.9 Mbps |

No host or decoder-input drops were reported in the analyzed windows. Decoder
queues stayed at 0–2 frames. Median control round trips were about 2–3 ms in both
modes. The slowdown in the Cairo runs was already present in the workload's draw
rate; the tablet and USB link were not falling behind the encoded stream.

The GPU color-node workload was limited by the workload itself at roughly
30 fps in both modes. It does not establish a 30 fps limit for GPU applications:
the GPU scrolling workload reached 60 fps at native resolution.

## Method and limits

- Samsung SM-X510 tablet, 2304 × 1440 panel, Android 16, USB connection.
- GNOME Wayland with the recorded patched Mutter 51.0-2 loaded by GNOME Shell.
- Production capture and session code, DMA-BUF import, `vah264lpenc`, GStreamer
  1.28.8, a 60 fps ceiling, and the existing 6–50 Mbps adaptive bitrate bounds.
- Eight sessions: current/native/native/current, repeated with a second renderer.
  Each session used 4 seconds of static text, 12 seconds of scrolling, 12 seconds
  of full-screen motion, and 4 seconds of recovery. The first and last 2 seconds
  of each motion phase were excluded from the analysis.
- The first fixture used Cairo drawing. The second used GTK snapshot color and
  text nodes through `GskVulkanRenderer`, avoiding a full-frame Cairo raster.
  GNOME exposed scale 1 to the current GPU fixture and scale 2 to the native GPU
  fixture, with the same logical window size and a 60 Hz virtual monitor.
- The camera setting remained enabled throughout. The host reported that
  `/dev/video10` was absent in every run, so these results do not validate a
  working camera export alongside the display.

Host pipeline time was measured from `appsrc` to `appsink` with
[GStreamer's latency tracer](https://gstreamer.freedesktop.org/documentation/coretracers/latency.html).
It includes the video processing path, not the entire delay from drawing a
desktop frame to light appearing on the tablet. The control round trip is a
separate measurement; neither number is glass-to-glass latency.

Tablet screenshots confirmed the text-size match and the native mode's sharper
small text. These are synthetic workloads on one machine, not measurements of
every application or of wireless performance. Background desktop activity and
the short runs introduce some variation.

## Tooling

The old FPS counter counted incoming health reports and therefore tended toward
2 fps even while video was smooth. It now divides the change in the tablet's
decoded-frame counter by elapsed time. Idle health reports no longer count as
video frames, and a decoder counter reset clears the measurement window.

The app also labels streamed resolution accurately, avoids dividing an already
reduced framebuffer a second time when describing the desktop workspace, and
calls the ping measurement a connection round trip.

`cargo run --release -p xs-core --example resolution_bench` runs a real tablet
session and emits JSON measurements using the current tooling. Close the desktop app before running it;
`-- --resolution native --transport wifi` selects native scaling and saved-pairing
Wi-Fi when the compositor check passes. Use `--transport usb --serial SERIAL`
for an explicit USB trial. The benchmark disables camera forwarding and leaves
saved app settings unchanged.
The experimental fixtures, raw logs, and screenshots were retained locally in
`/tmp/extraspace-resolution-ex2pqc2e`; no desktop captures were committed.
