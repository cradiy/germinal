# Snapshot cell-style reuse: 2026-10-05

Before: release revision `29d8a1b`. After: reuse the most recently resolved cell
style while constructing one render-surface snapshot. The lookup key includes
foreground/background colors, all cell flags, underline color, hyperlink
presence, and selection state. Character content and grid positioning are
processed as before. The lookup is recreated for each snapshot, so palette
updates do not require cross-frame invalidation. Only one entry is retained,
with no persistent allocation or additional history-dependent storage.

## Evidence and method

A pre-change 20,000-frame ASCII profile using
`perf record -e cpu-clock:u -F 499` attributed 12.85% of samples to
`color_to_rgb` and 46.09% to render-surface snapshot generation itself.
Profile: `/tmp/germinal-snapshot-before.perf`. This motivated avoiding repeated
color/style resolution on adjacent cells with identical styling.

Uninstrumented A/B runs used AMD Ryzen AI 7 H 350, Rust 1.98.0, release builds,
JetBrainsMono Nerd Font size 20, 142x40 cells, 10,000 scrollback lines, and disabled
ligatures. Each case had three paired repetitions of 100 warm-up plus 5,000
measured frames. Binary order alternated before/after, after/before, before/after.
No compilation or tests ran concurrently with formal sampling. DISPLAY and
WAYLAND_DISPLAY were unset. No windows, GPU devices, or PTY processes were used.

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile ascii --frames 5000 --repeats 1
```

The same command ran for all four profiles. The before binary was saved at
`/tmp/germinal-headless-before-cell-style`. [CSV](cell-style-2026-10-05.csv)
retains every measured CPU stage for all repetitions.

## Results

Median snapshot-generation time, microseconds per frame:

| Profile | Before | After | Reduction |
|---|---:|---:|---:|
| ascii | 51.579 | 45.761 | 11.3% |
| unicode-repeated | 49.788 | 44.980 | 9.7% |
| unicode-unique | 41.832 | 39.326 | 6.0% |
| links | 50.382 | 47.627 | 5.5% |

These are snapshot-stage improvements, not overall terminal CPU reductions.
The fixtures contain runs of uniform styling; per-cell alternating styles may
have different performance and were not benchmarked. CPU frequency and desktop
background activity were uncontrolled. This does not measure presentation,
input latency, GPU utilization, or displayed FPS.

## Validation

Workspace format/check/tests and all-target/all-feature Clippy with warnings
denied passed: 617 tests passed, one ignored. The terminal-store tests include
selection, hyperlink decoration, colors, and wide-character behavior. A new
regression verifies that normal/inverse/normal runs retain their correct colors
after an OSC palette update between snapshots.

The implementation changes only style resolution in the snapshot path.
Real-window visual acceptance remains deferred to avoid desktop interruption;
headless results are not a claim of compositor or visual validation.
