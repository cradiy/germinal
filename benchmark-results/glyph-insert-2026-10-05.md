# Duplicate glyph collection: 2026-10-05

Before: release revision `79970cf`. After: glyph collection accepts a borrowed
segment and checks its key before copying it or generating cursor fallback
segments. Already-collected glyphs return immediately. Fallback generation runs
once per distinct shaped glyph. No new cache or lifetime extension is introduced.

The pre-change 20,000-frame unique-Unicode profile attributed 5.01% of sampled
CPU to `insert_text_segment`, with B-tree operations also among the hotspots.
Sampling used `perf record -e cpu-clock:u -F 499`; raw data are at
`/tmp/germinal-glyph-insert-before.perf`.

## Measurement

AMD Ryzen AI 7 H 350, Rust 1.98.0, release builds, JetBrainsMono Nerd Font size 20,
142x40 cells, 10,000 history lines, ligatures disabled. Each case used three
paired repetitions, 100 excluded warm-up frames and 5,000 measured frames.
Order alternated before/after, after/before, before/after. No tests or compilation
ran concurrently. DISPLAY and WAYLAND_DISPLAY were unset; no desktop windows,
GPU devices, or PTY subprocesses were created.

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile unicode-unique --frames 5000 --repeats 1 --lines-per-frame 1
```

The full-replacement case uses 40 lines per frame. Saved baseline binary:
`/tmp/germinal-headless-before-glyph-insert`.
All individual stage measurements are in [the CSV](glyph-insert-2026-10-05.csv).

Median CPU atlas-preparation time in microseconds per frame:

| Scenario | Before | After |
|---|---:|---:|
| ascii | 0.511 | 0.527 |
| unicode-repeated | 22.122 | 21.466 |
| unicode-unique | 43.987 | 39.185 |
| links | 0.628 | 0.672 |
| unicode-unique-full | 104.376 | 100.116 |

Unique Unicode scrolling improves by 10.9%; full replacement by 4.1%.
Repeated Unicode improves by 3.0%, a smaller effect. ASCII and links show small
absolute increases of 0.016 and 0.044 us respectively; their core-ASCII shortcut
is unchanged. These measurements are stage timings, not whole-terminal CPU or
displayed FPS. Background load and CPU frequency are uncontrolled. Ligature
performance was not benchmarked.

## Validation

Workspace format/check/tests and all-target/all-feature Clippy with warnings
denied passed. Regression coverage compares collected segments against the
original fallback-first algorithm using repeated ligatures, emoji sequences,
Arabic text, and both normal/bold styles; required cursor fallback characters
remain present. Existing atlas cache, font-style, and preedit tests passed.

All runtime sampling was headless. Window/compositor rendering and cursor
visual acceptance remain deferred; compilation and headless tests do not
establish visual correctness.
