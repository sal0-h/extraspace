# Sharpness experiment plan

Written October 19, 2026, for the Samsung SM-X510 at 1.5× over saved-pairing
Wi-Fi. This is a historical plan. The [results](../benchmarks/sharpness.md)
record what was implemented and measured.

## Status

| Work | Outcome |
|---|---|
| Explicit Standard/native and USB/Wi-Fi benchmark selection | Implemented |
| Repeatable text, scrolling, motion, and recovery fixture | Implemented |
| Three alternating trials per resolution | Completed with a GPU fixture |
| Capture/write gaps, drops, and load measurements | Added; limits are in the results |
| Encoded-reference and DMA-BUF lifetime fixes | Implemented and recovery-tested |
| Image-quality setting in the installed app | Implemented; checked through launcher and saved Wi-Fi |
| User reading and scrolling comparison | Sharper text with comparable responsiveness at 1.5× |
| Matched-bitrate codec comparisons | Not performed |
| Physical input-to-display latency and battery measurements | Not performed |
| Fifteen-minute thermal/power follow-up and broad application survey | Not completed |
| Frame IDs carried into receiver feedback and confirmed presentation timing | Deferred |

## Experiment order

1. Compare Standard and native H.264 through the production session path. Keep
   text size, orientation, brightness, camera state, and network location fixed.
   Verify the running compositor before requesting native scaled modes.
2. Alternate three trials per mode, with at least a minute of motion. Use small
   black and colored text, thin lines, gray/color ramps, scrolling, and static
   recovery. Keep CPU/Cairo stress separate from GPU-rendered workloads.
3. Record frame gaps, host pipeline timing, write stalls, bitrate, decoder queues,
   recovery images, and load. Keep clock domains separate. Use a camera for
   physical latency if available; screenshots cannot establish it.
4. If H.264 meets the practical goal, integrate it with a Standard fallback and
   verify the installed launcher, Quick Settings, USB, and saved Wi-Fi. Investigate
   another codec or transport only if measurements identify a remaining limit.

## Proposed targets

These were experiment goals, not claims that every item was later measured.

| Property | Target |
|---|---|
| Text | Clearer small text at the same size, without washed-out colors |
| Motion | At least 57 decoded FPS when the source draws steadily at 60 FPS |
| Responsiveness | Pass the user's comparison; if physically measured, add no more than one 60 Hz refresh, about 17 ms, at median and p95 |
| Stalls | No repeatable increase in gaps over 50 ms or accumulating backlog |
| Recovery | No corruption or repeatable regression at startup, reconnect, or overload |
| Sustained use | Check thermal behavior and power over 15 minutes |

## Conditional work

If compression or network capacity remains limiting, compare encoder speed,
keyframe policy, and hardware H.265 at matched bitrate and text quality. Preserve
reference-frame recovery, GPU memory, and H.264 fallback.

If drawing limits the source, inspect application/compositor work. If Android
presentation limits output, compare decoder and surface behavior while retaining
cursor and touch. Datagram transport requires evidence of stale delivery under
loss, plus authentication, congestion control, and dependency-aware recovery.
Text/color refinement remains a larger experiment for persistent fidelity loss.
