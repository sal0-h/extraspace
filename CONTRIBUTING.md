# Contributing

File issues and pull requests at
[sal0-h/extraspace](https://github.com/sal0-h/extraspace). See [setup](docs/setup.md)
for host dependencies and builds, and [architecture](docs/architecture.md) for
the crate boundaries and session lifecycle.

## Checks

Run the Rust checks used by CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

GTK tests need a display, but no tablet or GNOME session. On a headless machine,
install Xvfb and xauth, then use:

```bash
GSETTINGS_BACKEND=memory xvfb-run -a cargo test --workspace --locked
```

Build Android changes with JDK 17:

```bash
(cd android && ./gradlew assembleRelease --no-daemon)
```

Check installer and packaging scripts:

```bash
for script in scripts/*.sh packaging/mutter-patch/*.sh \
  packaging/mutter-patch/PKGBUILD \
  packaging/mutter-patch/extraspace-mutter-patch-check; do
  bash -n "$script"
done
```

After building the release host, test windowless controls on a private session
bus. This needs Python GI bindings (`python3-gi` on Debian/Ubuntu):

```bash
GSETTINGS_BACKEND=memory GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- xvfb-run -a python3 scripts/test-background.py target/release/extraspace
node extension/tests/state.mjs
```

Test extension changes in a separate GNOME Shell session before loading them
into your desktop. Manual [integration examples](docs/architecture.md#integration-examples)
exercise real capture and require GNOME Wayland.

## Compatibility

Changes to `xs-proto` must also update
[Protocol.kt](android/app/src/main/kotlin/io/github/tymonoman/extraspace/Protocol.kt).
Build the host and companion from the same checkout. When an existing tablet
needs a companion upgrade, bump Android's `versionCode` and the host's
`APK_VERSION` together. Preserve application identifiers so updates retain
installed data and pairing.

Keep Mutter's private APIs and monitor configuration in `xs-mutter`, and keep
transport framing separate from session policy. Explain non-obvious decisions
in comments, such as ADB half-close behavior and encoder bitrate units. For
behavior changes, test the failing case and describe the trigger and result in
the commit message.

## Hardware testing and reports

Check the installed launcher and Quick Settings tile as well as development
launches. For connection or lifecycle changes, test saved Wi-Fi and USB,
Home/return on Android, reconnects, and full shutdown. For display changes,
check scale, quality, cursor, touch coordinates, and physical monitor layouts.

Use a [hardware report](https://github.com/sal0-h/extraspace/issues/new?template=hardware_report.yml)
to record a tablet model, Android version, GNOME version, distribution, encoder,
transport, and working features. Include the relevant logs described in
[troubleshooting](docs/usage.md#troubleshooting), but omit pairing credentials
and private desktop captures.
