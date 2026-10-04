# Bare-terminal baseline: 2026-10-05

Measured Germinal revision `0d03db5` (release build) against Kitty 0.48.2 on the user's Niri
desktop. GPU: NVIDIA GeForce RTX 5060 Laptop GPU, driver 615.71.09. Display: 2560x1600, 165 Hz,
scale 1.5, VRR enabled. Both fullscreen windows reported 142 columns and 40 rows, used
JetBrainsMono Nerd Font, 10,000 history lines, opaque backgrounds, and disabled cursor animation,
cursor blinking, and ligatures. User configuration was not modified.

Command, with desktop session variables set:

```sh
python3 scripts/terminal-compare.py run \
    --output /tmp/germinal-compare-1005-matched \
    --repeats 5 --seconds 5 --lines 2500000
```

There were 22 warm-up runs and 110 measured runs. Terminal order alternated between repetitions.
One fifth-round Kitty clear/redraw attempt lost focus, was excluded, and was successfully retried
using `--resume`. Raw CSV, producer logs, configurations, geometry, and warm-up screenshots are
in `/tmp/germinal-compare-1005-matched`. That location is temporary; archive it before cleaning
`/tmp`. Earlier calibration directories are not part of these results.

All table values are medians of five runs. CPU uses one core as 100%; RSS is the peak observed
resident memory, not private/PSS memory. VRAM is the median of each run's average PID framebuffer
memory reading from `nvidia-smi pmon`.

| Workload | Germinal CPU % | Kitty CPU % | Germinal RSS MiB | Kitty RSS MiB | Germinal VRAM MiB | Kitty VRAM MiB |
|---|---:|---:|---:|---:|---:|---:|
| Idle | 0.41 | 0.00 | 173.64 | 150.83 | 185 | 55 |
| Clear/redraw every 200 ms | 1.42 | 1.42 | 178.54 | 151.94 | 185 | 55 |
| ASCII paced at 165 lines/s | 12.40 | 6.71 | 178.96 | 155.29 | 185 | 55 |
| Unicode paced at 165 lines/s | 17.07 | 6.91 | 181.90 | 158.19 | 169 | 55 |
| ASCII flood | 111.99 | 121.28 | 216.91 | 195.28 | 185 | 55 |
| Unicode flood | 117.10 | 117.07 | 220.91 | 198.03 | 169 | 55 |
| Cursor position updates | 3.66 | 3.25 | 177.59 | 150.90 | 185 | 55 |
| Static RGBA | 1.39 | 0.20 | 182.96 | 158.90 | 159.4 | 93 |
| Static PNG | 0.41 | 0.20 | 178.31 | 159.19 | 153 | 93 |
| Animation, requested 8 ms | 12.89 | 8.96 | 197.43 | 170.84 | 153 | 91 |
| Animation, requested 16 ms | 10.54 | 5.97 | 197.47 | 170.77 | 153 | 91 |

For the same 2.5 million lines, lower flood completion time is better. CPU total includes the
sampled workload plus a three-second drain period.

| Flood | Germinal completion s (range) | Kitty completion s (range) | Germinal CPU total s | Kitty CPU total s |
|---|---:|---:|---:|---:|
| ASCII | 3.98 (3.92–4.03) | 3.38 (3.32–3.43) | 4.46 | 4.07 |
| Unicode | 6.90 (6.71–6.99) | 4.18 (4.12–4.24) | 8.01 | 4.89 |

The producer's own transmission timer excludes the image hold period. These times measure writes
to the terminal, not GPU upload completion or first visible presentation.

| Image transmission | Germinal ms | Kitty ms |
|---|---:|---:|
| Static RGBA | 31.474 | 7.759 |
| Static PNG | 0.031 | 0.029 |
| Animation, 8 ms interval | 186.603 | 48.466 |
| Animation, 16 ms interval | 188.983 | 50.797 |

The stable gaps are text CPU and baseline VRAM. Paced ASCII CPU is approximately 1.85 times
Kitty; paced Unicode is approximately 2.47 times. Unicode flood completion takes approximately
1.65 times as long, with approximately 1.64 times the sampled CPU time. Clear/redraw CPU is close.
Idle VRAM is approximately 3.36 times Kitty, while both idle runs have near-zero board activity.

Board power medians were 8.15/8.42 W idle, 17.76/18.41 W paced ASCII, 17.99/18.82 W paced Unicode,
and 18.78/13.92 W Unicode flood (Germinal/Kitty). Board readings include the compositor and other
desktop applications; they cannot be assigned entirely to the tested terminal. Process SM
utilization was available for only 61/280 Germinal and 48/267 Kitty workload samples. Available-only
averages in the raw summary are not reliable comparative GPU utilization estimates.

Measurement limits:

- Five-second timed runs give a short baseline, not sustained thermal or battery measurements.
- CPU/RSS sampling begins after detecting the worker's start marker, with approximately 20 ms
  polling granularity. Very short image-transfer CPU work can precede the first sample; the image
  CPU rows must not be interpreted as precise upload-cost measurements.
- Warm-up screenshot spot checks covered clear/redraw, Unicode, image placement, and animation
  frames. They do not establish frame pacing, complete animation frame order, or displayed FPS.
- `watch-clear` is synthetic; this does not validate actual `watch nvidia-smi` byte handling.
- Output-driven cursor positioning is not keyboard-to-display latency. Cursor animation was off.
- This baseline excludes Zellij, IME, fractional-scale variation, and other compositors.
- Test windows were checked for focus and geometry before and after sampling; continuous focus
  history was not recorded.

Next investigations should profile the Unicode PTY/snapshot/render path and account for concrete
GPU allocations. The retained renderer allocates two full-size textures; at this display size,
RGBA8 storage is roughly 31 MiB in total before allocator overhead. This explains only part of
the observed VRAM difference and is not a complete attribution of the remaining memory.

Validation: release build, workspace format/check/tests, and all-target/all-feature Clippy with
warnings denied passed. The runner completed on the real desktop, including interruption cleanup
and successful resume. No terminal rendering implementation was changed for these measurements.
