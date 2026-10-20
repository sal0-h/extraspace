# Using Extraspace

## Start and stop

Turn on **Extraspace** in GNOME Quick Settings to start the display without a
window. The tile shows the connection state; its dropdown opens **Settings**
or **Pair tablet**. Turning it off stops capture and networking, then exits
the host process. The extension does not poll or start the app while off.

Opening Extraspace from the application menu opens its window and starts
connecting. Right-click the launcher and choose **Settings** to change settings
without starting a display. Closing settings keeps an enabled display running;
closing it while the display is off exits the app.

## USB

Connect an authorized USB data cable, then turn Extraspace on. USB debugging
must be enabled; see [setup](setup.md#install-the-tablet-companion). The host
can install and launch the companion automatically.

Going Home on the tablet pauses the display. Reopening the tablet app resumes
the session over USB; recovery waits for you to return instead of forcing the
app onto the screen.

## Wi-Fi

Pair once while both devices are on the same local network:

1. On the laptop, choose **Pair tablet** from Extraspace's menu or the Quick
   Settings dropdown.
2. On the tablet, tap **Pair computer**.
3. Scan the laptop's QR code, or enter the grouped text code. Codes expire after
   five minutes and can be used once.

For later connections, turn Extraspace on at the laptop and open it on the
tablet. Saved pairing survives app and computer restarts. You do not need an
IP address, Android Wireless debugging, or another pairing code.

The devices connect directly over authenticated TLS. There is no cloud relay.
Being on a campus-wide Wi-Fi network does not guarantee connectivity: access
point isolation, subnets, and firewall rules can block direct connections.
Discovery uses DNS-SD; saved devices also try their last working address.

To revoke access, use **Paired devices > Forget** on the laptop or **Saved
computer > Forget computer** on the tablet. While streaming, Android's Back
gesture opens the saved computer controls. Uninstalling the tablet app removes
its pairing.

## Background and battery behavior

The tablet runs discovery and streaming only while Extraspace is visible.
Going Home or switching apps closes its network channels, decoder, and camera;
returning reconnects. The screen stays awake while streaming and follows its
usual timeout while waiting. Stored pairing keys do not require a background
service. Opening the laptop app over native Wi-Fi does not open a hidden
tablet app; USB's explicit ADB launch can do so.

## Display and image quality

Choose **Extend** for a new monitor or **Mirror** to copy an existing display.
Arrange an extended display in GNOME's **Settings > Displays**.

**Scale** changes text and control size. **Image quality** controls how many
pixels are streamed in Extend mode:

- Sharper uses near-native tablet resolution with GNOME scaling. It is the
  default when the [compositor compatibility check](../packaging/mutter-patch/README.md)
  passes.
- Standard sends a smaller frame that the tablet enlarges. Use it when an
  application draws slowly or the connection cannot handle the extra data.

Both settings keep roughly the same text size and workspace. Sharper applies
at every offered scale. For a 2304 × 1440 tablet:

| Scale | Standard stream | Sharper stream |
|---|---|---|
| 1× | 2304 × 1440 | 2304 × 1440 |
| 1.25× | 1842 × 1152 | 2300 × 1440 |
| 1.5× | 1536 × 960 | 2304 × 1440 |
| 1.75× | 1316 × 822 | 2296 × 1428 |
| 2× | 1152 × 720 | 2304 × 1440 |

Fractional scales sometimes trim a few pixels so GNOME can use whole logical
pixels. At 1×, both settings already use full resolution. Mirror mode follows
the source display's resolution. Changing quality, scale, or mode rebuilds the
display while retaining its tablet and transport, including Wi-Fi with USB
attached.

The statistics show streamed resolution, encoder, decoded frame rate, and
connection round trip. An idle desktop produces fewer frames because capture
follows screen changes. Round trip measures the connection, not full
input-to-display latency. See the [benchmarks](benchmarks/sharpness.md).

## Camera

Enable **Tablet Camera** to expose the tablet camera as **Extraspace Tablet
Camera** through `/dev/video10`. The host needs the optional
[virtual-camera setup](setup.md#host-dependencies). With `exclusive_caps=1`,
other apps see the webcam while Extraspace is sending camera frames.

## Troubleshooting

| Symptom | Check |
|---|---|
| USB tablet missing | Run `adb devices`, accept authorization, and try a USB data cable. |
| Wi-Fi tablet missing | Open both apps, check saved pairing, and check network isolation or firewall rules. |
| No H.264 encoder | Inspect `vah264lpenc`, `vah264enc`, `x264enc`, or `openh264enc` with `gst-inspect-1.0`. |
| Black screen or protocol mismatch | Update both apps from the same checkout and collect the host/tablet logs below. |
| Sharper unavailable | Check the Mutter patch version and log out/in after replacing the compositor library. |
| Camera missing | Check `/dev/video10`, the Tablet Camera switch, and Android camera permission. |

For host logs, quit the running host, then launch the installed binary from a
terminal:

```bash
RUST_LOG=debug ~/.local/bin/extraspace
```

For tablet logs when ADB is available:

```bash
adb logcat -d -s extraspace:V
```

Report bugs in [the repository's issues](https://github.com/sal0-h/extraspace/issues)
with your GNOME version, distribution, tablet model, Android version, transport,
and image quality setting. Do not attach `wireless.json`; it contains private
identity material.
