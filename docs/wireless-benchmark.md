# Wireless ADB comparison

This comparison checked whether the tablet can stream over Android's secure
wireless debugging connection at the same 1536×960 resolution and 60 Hz used
over USB. The test ran twice per transport in USB, Wi-Fi, Wi-Fi, USB order with
the USB cable left attached. A GPU-rendered moving desktop fixture ran during
each session. Results use the middle eight seconds of the scrolling and motion
phases. Android logs recorded frame-receive and decoder-output gaps over 50 ms.

| Workload | Transport | Decoded FPS | RTT p50 / p95 | Receive gaps >50 ms | Decoder-output gaps >50 ms |
|---|---|---:|---:|---:|---:|
| Scrolling | USB | 59.9 | 3.2 / 4.4 ms | 0 | 0 |
| Scrolling | Wi-Fi | 59.9 | 9.5 / 21.7 ms | 16 | 32 |
| Moving content | USB | 49.4 | 3.5 / 5.1 ms | 2 | 2 |
| Moving content | Wi-Fi | 49.4 | 10.9 / 17.5 ms | 1 | 1 |

Neither transport dropped frames in these intervals; the Android decode queue
peaked at one frame. During scrolling, Wi-Fi had receive gaps up to 70 ms and
decoder-output gaps up to 78 ms, while USB had no gaps over 50 ms. The moving
fixture itself ran near 50 FPS, which accounts for the lower rate in that phase.

Wi-Fi is practical for using the tablet without a cable, and the average frame
rate matched USB in this trial. Its frame timing and control round trips were
less consistent, so USB remains the smoother option for this setup. These
results describe one tablet and one local network; they do not predict other
routers or interference conditions. Camera forwarding was disabled during the
comparison.

To choose a paired Wi-Fi device while USB is attached, launch Extraspace with
the Wi-Fi serial shown by `adb devices`:

```bash
ANDROID_SERIAL=TABLET_IP:CONNECTION_PORT extraspace
```

If the selected Wi-Fi connection is unavailable, Extraspace reports an error
instead of silently switching back to USB.
