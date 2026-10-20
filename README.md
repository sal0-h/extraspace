# Extraspace

Use an Android tablet as a second monitor for GNOME over USB or Wi-Fi.

[![CI](https://github.com/sal0-h/extraspace/actions/workflows/ci.yml/badge.svg)](https://github.com/sal0-h/extraspace/actions/workflows/ci.yml)
[![License: GPL v3](https://img.shields.io/badge/License-GPLv3-blue.svg)](LICENSE)

![Extraspace running on an Android tablet](assets/demo.gif)

*Demo from the original project.*

Extraspace creates a virtual monitor you can arrange in GNOME Settings, or
mirrors an existing display. It supports touch input, a cursor rendered on the
tablet, and optional tablet-camera passthrough to Linux.

- Control the display from GNOME Quick Settings without keeping a window open.
- Pair over Wi-Fi once using a QR code or text code, then reconnect by opening
  the tablet app. Connections are direct and authenticated; no account or cloud
  service is involved.
- Use Sharper for near-native tablet resolution at your preferred text size,
  or Standard to reduce rendering and bandwidth costs.
- Capture and encode on the GPU with DMA-BUF and VA-API where available, with
  software H.264 encoding as a fallback.

## Requirements

- GNOME on Wayland and an Android 11+ tablet.
- GTK4, libadwaita, PipeWire, GStreamer, and an H.264 encoder.
- For source builds: Rust 1.96+, JDK 17, and the Android SDK.
- For USB: a data cable, ADB, and USB debugging enabled on the tablet.

The Quick Settings extension targets GNOME 51. Sharper currently requires the
[patched Arch Mutter package](packaging/mutter-patch/README.md); other setups
use Standard. Extraspace uses Mutter's private APIs, so compatibility can change
between GNOME releases.

## Install

Install the [host dependencies and Android build tools](docs/setup.md), then:

```bash
git clone https://github.com/sal0-h/extraspace.git
cd extraspace
(cd android && ./gradlew assembleRelease --no-daemon)
cargo build --release --locked
./scripts/install.sh
```

The installer adds the app launcher, companion APK, and Quick Settings extension
to your user account. It builds the host only if no release binary exists.
Follow the [setup guide](docs/setup.md) to install the companion on the tablet.

## Use

Turn on **Extraspace** in Quick Settings, or open it from the application menu.
For Wi-Fi, choose **Pair tablet** on the laptop and **Pair computer** on the
tablet, then scan the QR code or enter the code. After pairing, open the tablet
app whenever you want to connect. USB can launch the tablet app for you.

Arrange the display in **Settings > Displays**. Extraspace's settings control
Extend/Mirror mode, scale, image quality, and camera passthrough. Turning the
Quick Settings tile off stops streaming and exits the host process.

See the [usage guide](docs/usage.md) for connection behavior, battery use, and
troubleshooting.

## Documentation

- [Setup](docs/setup.md): dependencies, installation, updates, and removal.
- [Usage](docs/usage.md): USB, Wi-Fi pairing, display settings, and troubleshooting.
- [Contributing](CONTRIBUTING.md): builds, checks, and hardware reports.
- [Architecture](docs/architecture.md): session lifecycle, capture, and protocol.
- [Performance results](docs/benchmarks/sharpness.md): native-resolution Wi-Fi
  measurements and their limits.
- [Documentation index](docs/README.md): engineering notes and historical work.

Keyboard forwarding, stylus pressure, audio, and live tablet rotation are not
implemented.

## Credits and license

Maintained by [Salman Hajizada](https://github.com/sal0-h), based on
[Tymonoman/extraspace](https://github.com/Tymonoman/extraspace). The original
commit history and attribution are preserved.

[GPL-3.0-or-later](LICENSE).
