# Contributing

This repository is Salman Hajizada's fork of
[Tymonoman/extraspace](https://github.com/Tymonoman/extraspace). File issues and
pull requests against [sal0-h/extraspace](https://github.com/sal0-h/extraspace).

## Set up

Follow the [README](README.md#getting-started) to install host dependencies and
build both the Rust host and Android companion. The setup script uses Fedora
package names; use your own package manager on other distributions. Android
builds use JDK 17.

## Check a change

Run the same Rust checks as CI:

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets --locked -- -D warnings
cargo test --workspace --locked
```

For Android changes:

```bash
(cd android && ./gradlew assembleRelease --no-daemon)
```

For installer or packaging changes:

```bash
for script in scripts/*.sh packaging/mutter-patch/*.sh \
  packaging/mutter-patch/PKGBUILD \
  packaging/mutter-patch/extraspace-mutter-patch-check; do
  bash -n "$script"
done
```

Unit tests need no tablet or GNOME session. GTK tests need a display; on a
headless machine install Xvfb and xauth, then run
`GSETTINGS_BACKEND=memory xvfb-run -a cargo test --workspace --locked`.
The manual capture and simulated
tablet examples require GNOME on Wayland; see
[integration examples](docs/architecture.md#integration-examples).

After building, test the background application on a private session bus:

```bash
GSETTINGS_BACKEND=memory GSK_RENDERER=cairo GTK_A11Y=none \
  dbus-run-session -- xvfb-run -a python3 scripts/test-background.py target/release/extraspace
node extension/tests/state.mjs
```

The lifecycle script needs Python's GI bindings (`python3-gi` on Debian/Ubuntu).
The Quick Settings extension targets GNOME 51; test Shell integration in a
separate Shell session before loading a changed extension into your desktop.

## Keep the two halves compatible

If you change `xs-proto`, update
[Protocol.kt](android/app/src/main/kotlin/io/github/tymonoman/extraspace/Protocol.kt)
to match. Build both halves from the same checkout.

When the companion needs upgrading on existing tablets, update Android's
`versionCode` and the host's `APK_VERSION` together. Keep the existing application
identifiers compatible with installed copies.

## House style

Keep Mutter's private D-Bus API and display configuration code in `xs-mutter`.
Keep transport framing separate from session policy.

Comments should explain decisions that are hard to infer from the code, such as
ADB half-close behavior or encoder bitrate units. Describe the trigger and
result of a fix in the commit message. For behavior changes, prefer tests that
exercise the failing case.

The [architecture guide](docs/architecture.md) maps the crates and session flow.
The [engineering notes](docs/engineering-notes.md) preserve earlier debugging
results and measurements.

## Hardware reports

A [hardware report](https://github.com/sal0-h/extraspace/issues/new?template=hardware_report.yml)
should include the tablet model, Android version, GNOME version, distribution,
encoder, and which features worked. Partial results are useful too.

For bugs, include the host log from
`RUST_LOG=debug ./target/release/extraspace` and, when relevant, the tablet log from
`adb logcat -d -s extraspace:V`.
