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
- Experimental streaming through Android's secure wireless debugging ADB
  connection.
- Arch packaging for the Mutter virtual-monitor crash guards, currently targeting
  Mutter 51.0.

## Requirements

- GNOME on Wayland. Extraspace uses Mutter's private D-Bus APIs; compatibility
  needs checking on each GNOME release.
- An Android 11+ tablet with debugging enabled. USB needs a data cable; secure
  Wireless debugging is an experimental alternative.
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

## Experimental Wi-Fi connection

Extraspace can carry the screen stream over Android's secure wireless debugging
connection. Pair the computer in **Settings > Developer options > Wireless
debugging > Pair device with pairing code**, then connect using the device IP
and connection port shown on the Wireless debugging screen:

```bash
adb pair TABLET_IP:PAIRING_PORT
adb connect TABLET_IP:CONNECTION_PORT
adb devices
ANDROID_SERIAL=TABLET_IP:CONNECTION_PORT extraspace
```

The pairing port and connection port are different. `ANDROID_SERIAL` selects the
Wi-Fi connection when USB is also attached; without it, Extraspace uses the
first ready ADB device. Wi-Fi performance depends on the network. In a short
comparison at 1536×960 and 60 Hz, Wi-Fi held the average frame rate but showed
more frame-timing spikes and higher round-trip latency than USB. See the
[wireless comparison](docs/wireless-benchmark.md).

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

The unit tests run without a tablet or an active GNOME session. For architecture,
crate responsibilities, and manual integration checks, see
[architecture](docs/architecture.md) and [contributing](CONTRIBUTING.md).

Keyboard passthrough, stylus pressure, audio, and live tablet rotation remain
planned work.

## Credits and license

Originally created by [Tymonoman](https://github.com/Tymonoman). This fork is
maintained by [Salman Hajizada](https://github.com/sal0-h), with upstream
attribution and commit history preserved.

[GPL-3.0-or-later](LICENSE).
