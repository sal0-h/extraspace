# Native-resolution streaming on Salman's setup

Measured October 19–20, 2026, on the Samsung SM-X510, Intel Raptor Lake-P laptop,
GNOME/Mutter 51.0-2 with the recorded compositor patch, and native paired Wi-Fi.
USB was attached for tablet logs; the benchmark explicitly selected Wi-Fi.

The app now offers **Image quality → Sharper / Standard**. Sharper is preferred
when the running compositor passes the existing compatibility check. At 1.5×,
it streams 2304 × 1440 while keeping the same 1536 × 960 logical workspace.
Standard retains the previous 1536 × 960 stream enlarged on the tablet. Changing
quality rebuilds the display while retaining the selected device and transport.

The installed app was checked through its existing launcher: it started at native
resolution without a window, switched Standard → Sharper while retaining USB,
and opened/closed settings without restarting the stream. Unplugging USB then
reconnected over saved-pairing Wi-Fi at 2304 × 1440. Salman reported that text
was sharper and felt as responsive. The final saved setting is Sharper, 1.5×,
Extend, camera off; pairing and bitrate bounds were retained.

## Performance and limits

Six alternating 110-second trials used a GTK/GSK Vulkan fixture: 12 seconds of
static text, 64 seconds of scrolling, 16 seconds with a moving rectangle, and
8 seconds of static recovery. The text includes small black text, colored code,
thin lines, and gray ramps. Three runs used each resolution, in current/native/
native/current/current/native order. Camera forwarding was disabled.

The initial comparison used the encoded-reference fix described below, before
the final PipeWire ownership fix. The final native run includes that ownership
fix and an intentional 500 ms sender pause near the beginning of scrolling.
The steady window trims three seconds from each end of scrolling. The injected
pause precedes this window and is assessed separately for recovery.

| Metric | Standard, mean of 3 | Native, initial mean of 3 | Native, final run |
|---|---:|---:|---:|
| Decoded FPS, steady scrolling | 59.84 | 59.87 | 59.75 |
| Host pipeline median | 4.71 ms | 7.92 ms | 7.85 ms |
| Host pipeline p95 | 6.03 ms | 8.84 ms | 8.51 ms |
| Encoded bandwidth | 25.42 Mbps | 47.42 Mbps | 35.61 Mbps |
| Streaming-process CPU, percent of one core | 12.37% | 15.16% | 14.23% |

The final run's lower bitrate reflects adaptive backoff after the injected
stall, not a demonstrated compression improvement. The codec, keyframe interval,
and bitrate bounds remain H.264, 24 frames, and 6–50 Mbps. The extra pixels cost
roughly 3 ms in the measured host pipeline. Hardware video-engine busy time
roughly doubled in the initial comparison; this is not a battery-power measure.

Wi-Fi jitter occurred in both modes. Across the full scrolling interval,
Standard runs averaged 59.83–59.88 FPS and the initial native runs 59.49–59.89
FPS. One Standard run logged a 273 ms receive gap; the last initial native run
had a 210 ms video write stall, raw-frame drops, and bitrate adaptation near the
end of scrolling. Trimming the analysis window excludes that native incident,
so the steady table must not be read as proof of hitch-free wireless output.

Pipeline time is appsrc-to-appsink timing from the GStreamer latency tracer.
It excludes application drawing, transmission, decoding, and panel presentation.
CPU figures cover the streaming process, excluding GNOME and the fixture.
Decoder counters count buffers released to the surface, not proven panel
presentations. Clock domains were not combined to claim glass-to-glass latency.

This fixture does not validate every browser/editor or battery life. The earlier
[USB comparison](resolution-benchmark.md) still documents a CPU/Cairo workload
that falls near 31 FPS at native resolution before transmission. Standard is
available for workloads where native drawing remains too expensive.

## Correctness fixes required before enabling Sharper

The first native trial left blocks on a static page after one encoded frame
was discarded. Later P-frames can refer to that frame; another keyframe may
never arrive while the desktop is idle. The bounded encoded channel now applies
backpressure rather than discarding reference frames. Stale raw frames are
discarded before encoding. Closing the channel explicitly wakes a blocked
encoder callback during teardown. Android similarly waits briefly for codec
input capacity and reconnects after a persistent stall rather than silently
discarding a reference and continuing the prediction chain.

A later recovery capture still showed garbled text after a network stall.
Review found a separate ownership hazard: duplicating a DMA-BUF fd preserves
the allocation but does not prevent PipeWire from reusing its pixels. The
capture path now retains the dequeued PipeWire buffer until the GStreamer
memory is released, including across shallow buffer copies. Requeueing occurs
on the owning PipeWire loop, with generation checks for removed/replaced
buffers. This follows PipeWire's requirement to recycle a capture buffer only
after processing it.
[PipeWire stream buffer lifecycle](https://docs.pipewire.org/group__pw__stream.html)

After this repair, short recovery trials produced clean static captures: one
under concurrent build load, and two with confirmed 500 ms sender stalls near
the end of motion. The full-length final trial also injected a 500 ms stall,
dropped 29 raw frames, and recovered to a clean static page with an empty decoder
input queue. An independently decoded complete sent bitstream from a repaired
short trial also had clean recovery text. The original late corruption was not
captured as a complete bitstream, so that occurrence alone does not establish
which component first corrupted its pixels.

Regression checks cover shared-memory lease lifetime, raw-frame backpressure,
stopping a blocked encoder, explicit Wi-Fi selection, and saved quality controls.
Android logs identify `c2.exynos.h264.decoder`, hardware acceleration, and
advertised low-latency support on this tablet.

## Reproduction and diagnostic artifacts

Build `resolution_bench`, close the desktop app, and leave the tablet app open:

```sh
cargo build --release -p xs-core --example resolution_bench
target/release/examples/resolution_bench --resolution native --transport wifi --seconds 110
```

Once streaming, run `GSK_RENDERER=vulkan python3 scripts/sharpness-fixture.py`.
The fixture emits phase names and actual snapshot counts; send `mark NAME`
through benchmark stdin to align phases. `--quick` is a short recovery fixture,
not a substitute for the full performance run. Use `--fixed-kbps` for later
matched-bitrate experiments and `--transport usb --serial SERIAL` for USB.

`EXTRASPACE_VIDEO_TRACE=NEW_FILE` explicitly records video access units into a
new private file. `EXTRASPACE_VIDEO_TEST_STALL_MS` and
`EXTRASPACE_VIDEO_TEST_STALL_FRAME` inject one bounded diagnostic sender pause.
Leave these unset for normal launches. Captures contain desktop pixels.

Raw logs and captures remain local under `/tmp/extraspace-sharpness-*`, notably
the `20261019-233416` comparison, `20261019-235926` repaired recovery tests, and
`20261020-000149` final run. Desktop captures and bitstreams are not committed.
No H.265, datagram transport, or text-layer implementation was needed for this
first native-resolution result; those remain conditional experiments in the
[research notes](sharpness-research.md).
