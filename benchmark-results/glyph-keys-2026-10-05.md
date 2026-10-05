# Glyph-key collection: 2026-10-05

Before: release revision `304ca54`. After: Crossfont glyph keys are collected
directly into a vector, extended with core ASCII keys, sorted, and deduplicated.
This avoids constructing a temporary B-tree set before creating the cache-key
vector. Key ordering and uniqueness use the same key comparison. Debug glyph
collection retains its existing set-based behavior. No new cache is introduced.

## Measurement

AMD Ryzen AI 7 H 350, Rust 1.98.0, release builds, JetBrainsMono Nerd Font size 20,
142x40 cells, 10,000 history lines, ligatures disabled. Three paired repetitions
per case with 100 warm-up and 5,000 measured frames. Binary order alternated
before/after, after/before, before/after. No compilation or tests ran concurrently.
DISPLAY and WAYLAND_DISPLAY were unset; no windows, GPU devices, or PTY child
processes were created.

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile unicode-unique --frames 5000 --repeats 1 --lines-per-frame 1
```

Full replacement uses 40 lines per frame. Baseline binary:
`/tmp/germinal-headless-before-glyph-keys`. All stage timings and repetitions
are in [the CSV](glyph-keys-2026-10-05.csv).

Median CPU atlas-preparation time, microseconds per frame:

| Scenario | Before | After |
|---|---:|---:|
| ascii | 0.707 | 0.729 |
| unicode-repeated | 21.493 | 14.437 |
| unicode-unique | 39.504 | 32.124 |
| links | 0.932 | 0.901 |
| unicode-unique-full | 99.891 | 94.569 |

Repeated Unicode scrolling improves by 32.8%, unique Unicode by 18.7%, and
full replacement by 5.3%. ASCII and link cases use the unchanged core-ASCII
shortcut and show small absolute differences. These are stage timings, not
overall CPU reductions or displayed FPS. Background load and CPU frequency
remain uncontrolled; ligature-enabled performance was not benchmarked.

## Validation

Workspace format/check/tests and all-target/all-feature Clippy with warnings
denied passed. The 14 focused atlas tests cover cache reuse/invalidation,
distinct targets, ASCII prewarming, Unicode styles, preedit, and cursor
fallback collection. Existing regression coverage was retained without adding
a test that merely repeats standard-library sorting behavior.

All measurements were headless. Real-window visual and compositor acceptance
remain deferred to avoid interrupting the user's desktop.
