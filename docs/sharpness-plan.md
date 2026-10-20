# Plan for sharper text

The goal is visibly clearer text on Salman's Samsung SM-X510 over the existing
paired Wi-Fi connection, with responsiveness close to today's 1536 × 960 mode.
Keep the 1.5× UI size and normal Quick Settings / app-launcher workflow.
This is a plan; no stream settings or installed binaries changed while writing it.

The [research](sharpness-research.md) and [USB comparison](resolution-benchmark.md)
already show that native 2304 × 1440 can sustain about 60 decoded FPS with a
GPU-rendered fixture. Native resolution is the first candidate. A new codec is
an experiment to justify with measurements, not a prerequisite.

## 1. Establish where detail and time are lost

Extend the existing `resolution_bench` example rather than creating another
streaming implementation. Give it explicit current/native and USB/saved-Wi-Fi
selection, and record the actual selected transport, encoder, dimensions,
logical scale, and bitrate. Today's example inherits transport selection and
uses a process-wide environment flag for the native mode; production UI does
not need to expose those details.

Before a native trial, verify that the running compositor passes the existing
patched-Mutter check. Keep that check in place. A newer installed package alone
does not establish that the running compositor contains the required fix.

Use a fixed desktop page with small black text, colored code, thin lines, and
gray/color ramps, plus repeatable scrolling. Compare the captured source,
decoded stream, and tablet screenshot at the same physical text size. Check
the stream-to-tablet size mapping. This separates enlargement blur, rendering,
chroma loss, compression, and unexpected presentation scaling. A screenshot
cannot by itself measure display latency or the panel's optical behavior.

Keep the existing host timing and tablet decoder counters, but add the missing
observations needed to identify a bottleneck:

- Frame sequence identifiers carried through to receiver feedback.
- Capture/encoder timings and encoded frame size, including keyframes.
- Video write stalls, frame age within each device, receive gaps, decoder
  queueing, and output timing; transport retransmissions where available.
- A surface presentation signal where available. The current `framesDecoded`
  counter and `renderedAtUs` are updated when a codec buffer is released to the
  surface, before proof that the panel displayed it.

Keep host and tablet clock domains separate unless an explicit clock-offset
estimate and its uncertainty are recorded. Measure visible input-to-display
delay with a high-frame-rate camera if available. Otherwise report component
timings and a user A/B judgement; do not label their sum as measured glass-to-
glass latency. Keep diagnostic captures local and opt-in.

## 2. Compare the current path with native H.264

Start with the current native Wi-Fi protocol, not the older wireless-ADB test
path. Hold tablet orientation, brightness, font size, camera-off setting, and
network location constant. Warm up the devices and alternate baseline/native
order. Run three paired trials with at least a minute of motion in each mode,
using the controlled page and ordinary browser, editor, and terminal workloads.
Use the CPU/Cairo fixture as a separate stress case, not as the sole predictor
of everyday performance.

First compare the two modes with normal adaptive bitrate settings to establish
the actual user experience. Then use fixed bitrate points on the controlled
fixture to distinguish pixel count from the app's resolution-dependent bitrate
policy. Use USB as a diagnostic baseline if native Wi-Fi regresses.

Proposed acceptance targets, relative to the repeated current-mode baseline:

| Property | Target |
|---|---|
| Text | Clearly sharper small text at the same physical size; no new washed-out colors |
| Sustained motion | At least 57 decoded FPS for a source drawing steadily at 60 FPS |
| Responsiveness | If directly measured, median and p95 visible delay increase by no more than one 60 Hz refresh, about 17 ms; also pass the user's A/B judgement |
| Stalls | No repeatable increase in gaps over 50 ms, and no accumulating video backlog |
| Recovery | No corruption or repeatable regression in first-image, reconnect, or overload recovery |
| Sustained use | No thermal throttling or material extra power cost during a 15-minute follow-up; quantify power only if the measurement supports it |

These are proposed experiment gates, not performance claims. If native H.264
passes in normal use, stop codec work and integrate that result. If a gate
fails, identify the responsible stage before choosing the next experiment.

## 3. Tune H.264 only where the measurements justify it

The first tuning candidates are keyframe bursts and video-aware bitrate
feedback. Before increasing today's 24-frame keyframe interval, implement and
test dependency-safe recovery when an encoded reference frame is discarded.
Later predicted frames must not continue from a reference the tablet missed.
Startup and idle reconnect still need a prompt fresh image.

Compare the current interval with a 120-frame maximum, changing one variable at
a time. Compare the current encoder speed setting with one faster setting,
then judge text quality at matched bitrate and responsiveness. Keep B-frames
disabled and the existing GPU-memory path intact. Do not assume fewer
keyframes reduce the average bitrate under CBR.

If video backs up while the separate control ping remains healthy, use frame
age and send/receive feedback to reduce bitrate before a long stall develops.
Keep native resolution fixed during this comparison so text size and detail do
not oscillate with connection health. Re-run overload and reconnect checks for
every change to dropping or refresh policy.

If the loss is application drawing or compositor scaling, inspect that path
instead. Neither H.265 nor a different network transport can restore frames
that were never drawn. If the tablet's presentation path is responsible,
experiment with decoder selection or `SurfaceView`, retaining working cursor
overlays, touch mapping, and lifecycle behavior.

## 4. Test hardware H.265 if compression remains limiting

Query the tablet's real decoder capabilities first: hardware implementation,
native resolution at 60 FPS, low-latency feature, and performance points when
provided. The laptop's hardware encoder is already available.

Add explicit codec capability negotiation and an H.264 fallback. Compare both
codecs at equal bitrate for quality, then at comparable text quality for
bandwidth and timing. Include decode cost, recovery, and sustained power/load;
choose H.265 only if it improves the measured result on this device pair.

## 5. Integrate the winner into the installed app

If native output passes, make it the preferred path for this verified setup
through normal app configuration, keeping the compositor compatibility check
and a recoverable fallback. If its cost remains noticeable, retain today's
default and offer one clear Sharper option. No terminal flags or new pairing
steps in everyday use. Validate the actual installed binary from Quick
Settings and the desktop entry, over both saved Wi-Fi and USB.

Stop once text is clearly better and the responsiveness target is met. Larger
changes stay conditional: encrypted datagrams only for demonstrated loss/
retransmission stalls, and a text/color refinement layer only if native
hardware video still cannot provide adequate text fidelity. Direct wireless,
stylus support, and remote access are outside this work.
