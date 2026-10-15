# Architecture

Extraspace has a Rust host and a Kotlin Android companion. The host creates or
captures a GNOME display, encodes its frames, and sends them through three
ADB-forwarded sockets. The tablet decodes video, renders the cursor, and sends
touch events and optional camera frames back.

```mermaid
flowchart LR
    GNOME["GNOME / Mutter"] -->|PipeWire frames| VIDEO["Rust / GStreamer"]
    VIDEO -->|H.264 over USB| TABLET["Android / MediaCodec"]
    VIDEO -->|Cursor metadata over USB| CURSOR["Android cursor overlay"]
    TABLET -->|Touch over USB| GNOME
    TABLET -->|Camera H.264 over USB| CAMERA["Rust / GStreamer"]
    CAMERA --> WEBCAM["Linux /dev/video10"]
```

## Crates

| Crate | Responsibility |
|---|---|
| `xs-proto` | Frame headers and message definitions, mirrored by Android's `Protocol.kt` |
| `xs-mutter` | Display creation, monitor layouts, input injection, and the patched-Mutter check |
| `xs-video` | PipeWire capture, cursor metadata, frame pacing, and H.264 encoding |
| `xs-transport` | ADB orchestration, companion installation, and framed sockets |
| `xs-camera` | Decode the tablet camera stream into a V4L2 loopback device |
| `xs-core` | Session lifecycle and adaptive bitrate control |
| `xs-ui` | GTK4/libadwaita interface and persisted user settings |

## GNOME session setup

For an extended display, the host:

1. Creates a RemoteDesktop session and reads its session ID.
2. Creates a ScreenCast session linked to that RemoteDesktop session. This link
   makes injected touch coordinates relative to the captured stream.
3. Calls `RecordVirtual` with `is-platform: true` and cursor metadata enabled.
4. Subscribes to `PipeWireStreamAdded` before starting the RemoteDesktop session.
5. Connects the capture pipeline to the PipeWire node, then finalizes the virtual
   monitor's layout.

A linked session starts through RemoteDesktop. Starting ScreenCast directly is
rejected by Mutter. Closing the virtual display restores the physical monitor
layout while preserving changes made during the session.

Mirror mode records an existing monitor instead of creating a virtual one.

## Capture and rendering

The preferred capture path requests DMA-BUF frames and passes GPU-backed buffers
through GStreamer to a VA-API encoder:

```text
PipeWire -> appsrc -> drop-only frame pacing -> vapostproc -> VA-API H.264 -> USB
```

When GPU encoding is unavailable, the host uses copied frames with
`videoconvert` and x264 or OpenH264. Encoder bitrate units are normalized in
`xs-video::encoder`.

Cursor position and bitmap metadata travel separately on the control channel.
Android draws that cursor over a `TextureView`, while MediaCodec handles video.
Pointer movement therefore does not need to produce another encoded frame.

Mutter produces frames when the desktop changes. Frame pacing drops excess
frames instead of filling idle periods with duplicates. The Android decoder has
a separate output-drain thread so the final frame can render even when no new
input arrives.

## Scaling and compositor guards

The default path divides the tablet panel dimensions by the selected scale,
rounds to even dimensions, and lets Android upscale the video.

With `XS_MUTTER_MODES=1`, a recorded patched Mutter build can instead create a
near-native framebuffer and apply a GNOME logical scale. Dimensions are trimmed
so the logical size stays integral.

`xs-mutter::patched` checks the recorded package version against pacman's local
database and checks whether GNOME Shell is still using a replaced library.
Without a matching patch marker, scaled modes remain disabled. The patches,
installation hook, and rollback instructions live in
[packaging/mutter-patch](../packaging/mutter-patch/README.md).

## Integration examples

Unit tests need neither a tablet nor GNOME. These manual examples require an
active GNOME Wayland session and exercise real capture or virtual displays:

```bash
cargo run -p xs-core --example fake_tablet
cargo run -p xs-video --example capture_test
cargo run -p xs-mutter --example virtual_monitor
```

`fake_tablet` performs the handshake, consumes video, reports decoder health, and
sends touch events through real sockets. It exercises the host pipeline but does
not verify USB transport or Android's decoder.

See the [engineering notes](engineering-notes.md) for earlier hardware results
and the capture, encoder, and ADB behavior that motivated this design.
