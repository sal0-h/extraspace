<div align="center">

# Extraspace

Turn an Android tablet into a second monitor for GNOME over USB, with touch and camera passthrough.

[![CI](https://github.com/sal0-h/extraspace/actions/workflows/ci.yml/badge.svg)](https://github.com/sal0-h/extraspace/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)
[![Rust](https://img.shields.io/badge/Rust-1.96%2B-orange.svg)](https://www.rust-lang.org)
[![GNOME / Wayland](https://img.shields.io/badge/GNOME-Wayland-4A86CF.svg)](https://www.gnome.org)

![Extraspace running on an Android tablet](assets/demo.gif)

<sub>Original upstream demo, recorded on the tablet.</sub>

</div>

Extraspace adds a virtual monitor to GNOME's display settings. Move windows onto
the tablet, arrange it beside your physical displays, and use its touchscreen.
The tablet can also send its camera back to Linux as a webcam through
`/dev/video10`.

This is [Salman Hajizada's fork](https://github.com/sal0-h/extraspace) of
[Tymonoman/extraspace](https://github.com/Tymonoman/extraspace). It keeps the
original project history and adds work on frame pacing, cursor rendering, GPU
capture, display scaling, and monitor layout preservation.

## What this fork adds

- Low-latency frame pacing and a cursor rendered on the tablet, so moving the
  pointer does not require encoding another video frame.
- GPU capture with DMA-BUF and VA-API encoding, with x264 or OpenH264 as the
  software fallback.
- Full-range desktop color and optional native-panel rendering with GNOME
  scaling.
- Physical monitor layout preservation when a virtual display opens or closes.
- Native Wi-Fi streaming with one-time QR or code pairing and saved device trust.
- Arch packaging for the Mutter virtual-monitor crash guards, currently targeting
  Mutter 51.0.

## Requirements

- GNOME on Wayland. Extraspace uses Mutter's private D-Bus APIs; compatibility
  needs checking on each GNOME release.
- An Android 11+ tablet with the companion app installed. USB additionally needs
  a data cable and USB debugging; native Wi-Fi works without debugging enabled.
- Rust 1.96+ for the dependencies in `Cargo.lock`.
- JDK 17 and the Android SDK to build the companion app.
- GTK4, libadwaita, PipeWire, and GStreamer development packages.
- GStreamer's PipeWire plugin and a usable H.264 encoder: VA-API, x264, or
  OpenH264.
- Optional: `v4l2loopback` and `/dev/video10` for camera passthrough.

Upstream recorded end-to-end results on GNOME Shell 50.3 with a Telekom T Tablet
running Android 15. Those measurements are in the
[engineering notes](docs/engineering-notes.md); they are historical results,
not a claim that every device or GNOME version has been tested.

## Getting started

### 1. Clone this fork and install host dependencies

```bash
git clone https://github.com/sal0-h/extraspace.git
cd extraspace
```

On Fedora, the setup script installs system packages and configures the virtual
camera. It needs `sudo` when making changes:

```bash
./scripts/setup.sh --check
./scripts/setup.sh
```

The script uses Fedora's `dnf` and RPM package names. On Arch, Ubuntu, or another
distribution, install the dependencies listed above with your package manager.
The native library packages and Ubuntu build dependencies are also listed in
[CI](.github/workflows/ci.yml). Camera passthrough requires a loaded
`v4l2loopback` module configured for `video_nr=10` and `exclusive_caps=1`.

### 2. Enable USB debugging

On the tablet, tap **Build number** seven times in **Settings > About tablet**,
then enable **USB debugging** in **Developer options**. Connect the cable and
accept the authorization prompt.

```bash
adb devices
```

Your tablet should appear with the state `device`. A charge-only cable will not
work.

### 3. Build both halves and run

Use JDK 17 for Gradle. If your default Java is newer, set `JAVA_HOME` to your
JDK 17 installation before building.

```bash
(cd android && ./gradlew assembleRelease --no-daemon)
cargo build --release --locked
./target/release/extraspace
```

Run from the repository root. Extraspace finds the locally built APK, installs it
over USB when needed, and launches the companion app. Building both halves from
the same checkout keeps the desktop and tablet protocols in sync.

Open **Extra Display** to start streaming. Arrange the new monitor in
**Settings > Displays**.

### 4. Install the desktop launcher

```bash
./scripts/install.sh
```

The installer copies the desktop binary, icon, launcher, and locally built APK
under `~/.local`. Open Extraspace from the app grid or run `extraspace` in a
terminal. It starts when you open it.

To uninstall:

```bash
./scripts/install.sh --uninstall
```

Your saved settings remain in `~/.config/extraspace`.

## Display scaling

The default scale is 1.5×. The normal path creates a smaller virtual framebuffer
and lets the tablet upscale it. For a 2000×1200 panel:

| Scale | Stream resolution |
|---|---|
| 1× | 2000×1200 |
| 1.5× | 1332×800 |
| 2× | 1000×600 |

For sharper text, the optional native-panel path keeps the framebuffer near the
tablet's full resolution and lets GNOME scale the UI. Dimensions may be trimmed
slightly to keep logical pixels integral.

This path requires the [patched Mutter build](packaging/mutter-patch/README.md)
and `XS_MUTTER_MODES=1`. Extraspace checks the installed patch marker and the
running compositor before enabling it. After a Mutter upgrade, it falls back to
the default path until the patched build is restored.

## Wi-Fi connection

Open Extraspace from your computer's application menu. Choose **Pair tablet**,
then open Extraspace on the tablet and tap **Pair computer**. Scan the QR code
with the tablet camera, or enter the grouped code in the tablet app. Both devices
must be on the same local network. Pairing codes expire after five minutes and
can be used once.

After pairing, opening both apps reconnects automatically. No IP addresses,
terminal commands, Android Wireless debugging, or repeat pairing are needed.
Wi-Fi discovery and streaming run only while the tablet app is visible. Going
Home or switching to another app stops them; returning reconnects automatically.
The screen stays awake while streaming and follows its usual timeout while
waiting. Saved pairing keys do not require a background service.

Trust remains saved across app and computer restarts. Choose **Paired devices >
Forget** on the computer or **Saved computer > Forget computer** on the tablet to
revoke it. While streaming, Android's Back gesture opens the saved computer
controls. Uninstalling the tablet app removes its saved pairing.

Wi-Fi uses TLS 1.3 with a pinned computer identity and a separate random tablet
credential. The tablet encrypts its credential with Android Keystore; the host
keeps its identity and approved-device hashes in a private configuration file.
Discovery does not grant permission. There is no account or cloud service.

The companion app must be installed once. An existing USB setup updates it
automatically; alternatively install the matching APK on the tablet. USB works
as before and can launch the companion without opening it yourself.

The desktop app's **Connection** row shows which connection is in use. Changing
scale or display mode reconnects to the same tablet over the same connection,
including Wi-Fi when a USB cable is also attached. During a USB session, going
Home pauses the display; reopening Extraspace on the tablet resumes over USB
automatically. Recovery waits for the tablet app instead of opening it for you.

Local network isolation or a firewall can prevent a connection. Allow Extraspace
on your private network if prompted. QR pairing includes the current network
address, and saved connections also try their last working address when
discovery is unavailable.

The earlier [wireless comparison](docs/wireless-benchmark.md) measured streaming
over wireless ADB, before native Wi-Fi pairing was added. It is evidence about
the network's performance, rather than a benchmark of the new TLS transport.

## Troubleshooting

| Symptom | Check |
|---|---|
| No tablet found | Run `adb devices`, accept USB authorization, and try a data cable. |
| No usable H.264 encoder | Check `gst-inspect-1.0 vah264lpenc`, `x264enc`, or `openh264enc`; install the missing GStreamer plugin. |
| Black screen or protocol mismatch | Rebuild both halves, then inspect `adb logcat -d -s extraspace:V` and the host debug log. |
| Camera missing from apps | Check `/dev/video10` and enable Tablet Camera. With `exclusive_caps=1`, the webcam appears while frames are being sent. |
| Native scaling ignored | Check the [Mutter patch instructions](packaging/mutter-patch/README.md), including the required logout after installation. |

For host logs:

```bash
RUST_LOG=debug ./target/release/extraspace
```

Report problems in [this fork's issues](https://github.com/sal0-h/extraspace/issues)
with your GNOME version, distribution, tablet model, and Android version.

## Development

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

The unit tests run without a tablet or an active GNOME session. GTK tests need a
display; on a headless machine use `GSETTINGS_BACKEND=memory xvfb-run -a cargo test --workspace --locked`
with Xvfb and xauth installed. For architecture,
crate responsibilities, and manual integration checks, see
[architecture](docs/architecture.md) and [contributing](CONTRIBUTING.md).

Keyboard passthrough, stylus pressure, audio, and live tablet rotation remain
planned work.

## Credits and license

Originally created by [Tymonoman](https://github.com/Tymonoman). This fork is
maintained by [Salman Hajizada](https://github.com/sal0-h), with upstream
attribution and commit history preserved.

[GPL-3.0-or-later](LICENSE).
