# Unicode text optimization: 2026-10-05

## Scope and environment

Before: release binary from `bfa7675`, saved as
`/tmp/germinal-before-unicode-optimization`. After: release build with frame-local
text-run deduplication in `glyph_atlas_frame.rs` and ASCII-span batching within
mixed Unicode input in the Alacritty terminal fork. No rendering scheduler,
cursor lifecycle, or persistent-cache changes are included.

Measurements used the same Niri desktop as the baseline: NVIDIA RTX 5060 Laptop,
2560x1600 at 165 Hz, scale 1.5, fullscreen 142x40 terminal cells, JetBrainsMono
Nerd Font size 20, 10,000 history lines, ligatures and cursor animation/blinking
disabled. Temporary configurations did not modify user settings. Workloads ran
as child PTYs, without desktop keyboard injection.

The A/B harness reused `scripts/terminal-compare.py::run_case`, substituting only
the Germinal executable. Each workload had one excluded warm-up and three
measured repetitions per binary, with before/after order alternating by
repetition: 10 warm-ups and 30 measured runs. Timed workloads lasted eight
seconds at 165 lines/s; floods used 2,500,000 lines. No profiler or compilation
ran during these measurements. Focus and geometry checks passed.

Raw artifacts: `/tmp/germinal-unicode-ab-1005`. Its `environment.json` records
binary SHA-256 hashes, revision, dirty paths, display configuration, and
parameters. `summary.json` retains every individual result; per-case directories
contain CPU/GPU samples, producer logs, and warm-up screenshots. The temporary
driver is `/tmp/germinal-unicode-ab.py`. These paths are not durable archives.

## Results

Values are medians of three measured runs. CPU uses one core as 100%. CPU totals
include the sampled workload and three-second drain period. Flood completion is
producer delivery time, not completed screen presentation.

| Metric | Before | After | Change |
|---|---:|---:|---:|
| Paced Unicode CPU | 17.775% | 13.988% | -21.3% |
| Unicode flood delivery | 6.738 s | 6.556 s | -2.7% |
| Unicode flood CPU total | 7.97 s | 7.62 s | -4.4% |
| Paced ASCII CPU | 12.227% | 12.103% | -1.0% |
| Clear/redraw CPU | 1.386% | 1.386% | approximately unchanged |
| Cursor updates CPU | 3.528% | 3.652% | +0.124 percentage points |

Individual measured values, in repetition order:

| Metric | Before runs | After runs |
|---|---|---|
| Paced Unicode CPU % | 17.267, 17.775, 17.900 | 14.369, 13.739, 13.988 |
| Unicode flood delivery s | 6.738, 6.825, 6.573 | 6.556, 6.595, 6.522 |
| Unicode flood CPU total s | 7.97, 7.99, 7.71 | 7.64, 7.62, 7.51 |
| Paced ASCII CPU % | 11.853, 12.358, 12.227 | 12.353, 12.103, 11.978 |
| Clear/redraw CPU % | 1.386, 1.386, 1.386 | 1.260, 1.386, 1.386 |
| Cursor updates CPU % | 3.403, 3.528, 3.529 | 3.652, 3.528, 3.653 |

The Unicode CPU improvement is consistent across all three pairs. Flood gains
are smaller, with overlapping unpaired ranges. ASCII differences are small and
change direction by pair. Cursor CPU total medians are 0.29/0.30 seconds; the
small increase is recorded rather than treated as a proven zero-cost regression.
Longer runs would be needed to characterize it reliably.

Paced Unicode peak RSS medians are 185.00/184.96 MiB; flood RSS is
221.26/220.89 MiB. Process VRAM remains 169 MiB for Unicode and 185 MiB for the
other measured workloads. This optimization does not address the baseline VRAM
gap against Kitty. Kitty was not rerun in this A/B experiment.

## Profiling evidence

Separate before/after runs used `perf record -e cpu-clock:u -F 499 --call-graph
dwarf,8192 -p PID`, with both terminal-worker and render timing logs enabled.
Paced Unicode ran for 15 seconds; flood profiling used eight million lines.
Raw profiles are in `/tmp/germinal-unicode-paced-{before,after}` and
`/tmp/germinal-unicode-profile-{before,after}`.

Across the 14 complete 165-frame timing intervals in each paced profile:

| Per-frame timing, median of interval averages | Before | After |
|---|---:|---:|
| Frame preparation | 393 us | 180.5 us |
| Glyph-atlas preparation | 314 us | 101.5 us |

Atlas preparation includes text segmentation and glyph collection even on atlas
cache hits. Frame-local deduplication avoids repeating that work for identical
text with the same bold/italic face. It borrows text only during collection and
does not retain a growing cache across frames.

In flood profiles, scalar `Handler::input` accounted for 18.98% before and 8.47%
after. Mixed text now batches ASCII spans while retaining scalar handling for
non-ASCII characters and insertion mode. The instrumented flood delivery times
were 21.638/20.405 seconds; use the uninstrumented A/B table for performance claims.

## Validation and limits

- Workspace format/check and all-target/all-feature Clippy with warnings denied
  passed. Workspace tests passed on the host: 615 passed, one ignored. The initial
  sandbox run was blocked by communication-runtime permissions.
- The excluded Alacritty fork was tested separately: 136 unit tests, 45 reference
  replays, and one doctest passed. New differential coverage compares mixed-input
  batching with scalar input across narrow widths, wide-cell overwrites, wrap
  modes, combining characters, emoji, and insertion mode.
- Glyph collection coverage checks repeated Unicode text across all four
  bold/italic combinations, normalized combining characters, and IME preedit.
- Warm-up screenshots were inspected for Unicode text, clear/redraw, and cursor
  updates. No corruption or cursor trails were visible in those snapshots.
- Deduplication benefits repeated on-screen text. These synthetic workloads do
  not establish an equivalent improvement for all Unicode applications or
  predominantly unique text runs.
- Screenshots are spot checks, not continuous visual, frame-pacing, or input
  latency acceptance. Cursor animation was disabled. `watch-clear` is synthetic,
  not actual `watch nvidia-smi`; Zellij, IME interaction, and other compositors
  were not exercised in this run.
- GPU SM counters are incomplete, and board activity includes the desktop.
  No comparative GPU-utilization claim is made.
