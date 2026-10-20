# Documentation

## Using Extraspace

| Guide | Contents |
|---|---|
| [Setup](setup.md) | Requirements, source builds, installation, updates, and removal |
| [Usage](usage.md) | Quick Settings, USB, Wi-Fi pairing, image quality, and troubleshooting |
| [Mutter patch](../packaging/mutter-patch/README.md) | Arch package for native-resolution scaling, upgrade checks, and rollback |

## Development

| Guide | Contents |
|---|---|
| [Contributing](../CONTRIBUTING.md) | Build checks, integration tests, and bug reports |
| [Architecture](architecture.md) | Crates, capture pipeline, connection lifecycle, and saved trust |
| [Engineering notes](engineering-notes.md) | Buffer ownership, frame pacing, encoder behavior, and ADB pitfalls |
| [Sharpness results](benchmarks/sharpness.md) | October 19–20, 2026 native-resolution Wi-Fi comparison and recovery tests |

## Historical work

These documents record earlier experiments and decisions. Their settings and
recommendations describe the code at the time.

- [USB resolution comparison](archive/resolution-benchmark.md), October 14, 2026.
- [Wireless ADB comparison](archive/wireless-benchmark.md), before native Wi-Fi.
- [Sharpness research](archive/sharpness-research.md), October 19, 2026.
- [Sharpness experiment plan](archive/sharpness-plan.md), with completion status.
- [Filed Mutter crash report](../packaging/mutter-patch/UPSTREAM-REPORT.md),
  August 26, 2026. The report body is retained as filed.
