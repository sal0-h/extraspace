# Setup

Extraspace runs on GNOME Wayland with an Android 11+ companion. The Quick
Settings extension targets GNOME 51. The current native-resolution tests use
an Intel VA-API laptop, patched Mutter 51.0-2, and a Samsung SM-X510 on Android 16.
See the [performance results](benchmarks/sharpness.md) for that setup.

## Host dependencies

Install GTK4, libadwaita, PipeWire, GStreamer, their development headers, a C
compiler, Clang/libclang for the PipeWire bindings, and pkg-config. GStreamer
needs its PipeWire plugin and at least one H.264 encoder: VA-API (`vah264lpenc` or `vah264enc`), x264, or OpenH264. USB also
needs ADB from Android platform-tools or your distribution's `android-tools`
package.

On Fedora, the setup script checks dependencies and configures the optional
virtual camera. Installation uses `sudo`:

```bash
./scripts/setup.sh --check
./scripts/setup.sh
```

This script uses `dnf` and RPM package names. On other distributions, install
the dependencies with your package manager. Ubuntu build packages are listed
in [CI](../.github/workflows/ci.yml). Arch provides the VA-API GStreamer plugin
in `gst-plugin-va`.

Camera passthrough requires `v4l2loopback` loaded with `video_nr=10` and
`exclusive_caps=1`, exposing `/dev/video10`. The display works without it.

## Build and install

Install Rust 1.96+, JDK 17, and the Android SDK with platform 35 and
platform-tools. Configure the SDK using `ANDROID_HOME` or
`android/local.properties`. If your default Java differs, set `JAVA_HOME` to the JDK 17 installation before running Gradle.

From the repository root:

```bash
(cd android && ./gradlew assembleRelease --no-daemon)
cargo build --release --locked
./scripts/install.sh
```

The installer copies the release binary, launcher, icon, companion APK, and
Quick Settings extension under `~/.local`. It uses an existing release binary
if present, so rebuild before installing an update. A new extension may need
one logout/login before GNOME Shell discovers it.

For a development launch without installing:

```bash
./target/release/extraspace
```

Run this from the repository root so the app can find the built APK.

## Install the tablet companion

For automatic installation, enable USB debugging in the tablet's Developer
options, connect a data cable, and accept the computer's authorization prompt.
Developer options are enabled by tapping **Build number** seven times; its
location varies by device.

```bash
adb devices
```

The tablet should appear as `device`. Open Extraspace on the laptop to install
or update the companion and launch it. Use the [usage guide](usage.md) to set
up Wi-Fi afterward. Native Wi-Fi does not require Wireless debugging or ADB.

You can also sideload `android/app/build/outputs/apk/release/app-release.apk`
on the tablet and pair over Wi-Fi without using USB. Keep both apps from the
same checkout so their protocols match.

## Native-resolution scaling

Sharper requires the [Mutter patches](../packaging/mutter-patch/README.md).
The supplied package targets Arch and Mutter 51.0; it replaces a system package
and requires a logout/login. Other distributions need their own patched build
and compatibility support. Extraspace uses Standard when the check fails.

## Update or remove

To update, rebuild both apps, rerun `./scripts/install.sh`, and restart
Extraspace. Connect the tablet by USB for the automatic companion update, or
sideload the new APK. Updating retains settings and pairing.

To remove the host app and extension:

```bash
./scripts/install.sh --uninstall
```

Settings and pairing remain in `~/.config/extraspace` (or under
`XDG_CONFIG_HOME`). This does not remove the tablet app, host dependencies,
or a patched Mutter package. The [patch guide](../packaging/mutter-patch/README.md#rollback)
covers Mutter rollback.
