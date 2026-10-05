# Previous-frame text segmentation reuse: 2026-10-05

Before: release code at `39c3890` plus the headless benchmark example.
After: the same code with a per-render-target cache of text segments from the
current visible frame. Cache keys include exact text and bold/italic face.
The builder's ligature setting is immutable. Preedit is processed independently.

Entries are moved from the previous frame into the current frame when reused.
Unreferenced entries are dropped after collection; target removal drops its
text cache, and the ASCII atlas shortcut clears it. Storage is bounded by the
current visible runs, not scrollback history. During collection both old and
new maps can exist. This exchanges additional viewport-sized CPU storage for
less repeated segmentation; it does not reduce GPU memory.

## Method

AMD Ryzen AI 7 H 350, Rust 1.98.0, release builds, JetBrainsMono Nerd Font at size
20, ligatures off, 142x40 grid, 10,000 history lines. DISPLAY and WAYLAND_DISPLAY
were unset. No window, GPU device, or child PTY was created.

Three paired repetitions per case, 100 warm-up frames and 3,000 measured frames
per repetition. Binary order was before/after, after/before, before/after.
Cases ran sequentially with no concurrent compilation or tests.

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile unicode-unique --frames 3000 --repeats 1 --lines-per-frame 1
```

The full-replacement case uses `--lines-per-frame 40`, replacing all visible
lines every frame. The same fixed glyph vocabulary avoids repeated font-cache
misses. All measured atlas lookups hit; all unchanged-content shortcut counts
were zero. Individual data are in [the CSV](segment-cache-2026-10-05.csv).

## Results

Median of per-run means, CPU atlas preparation in microseconds per frame:

| Workload | Before | After | Change |
|---|---:|---:|---:|
| ASCII scrolling | 0.517 | 0.522 | approximately unchanged |
| Repeated Unicode scrolling | 28.198 | 22.326 | -20.8% |
| Unique Unicode scrolling | 99.136 | 43.734 | -55.9% |
| Links scrolling | 0.637 | 0.600 | small absolute difference |
| Unique Unicode full replacement | 101.952 | 106.571 | +4.5% |

The cache helps retained visible runs even when every line differs from other
lines. Full replacement cannot reuse those runs and pays cache maintenance
cost. These measurements demonstrate that tradeoff rather than a universal
Unicode speedup. Other stage timings are retained in the CSV; the change does
not target parsing, snapshot generation, or link discovery.

The pre-change headless profile used `perf record -e cpu-clock:u -F 499` over
20,000 unique-Unicode frames. Grapheme iteration accounted for 7.33% of sampled
CPU, text segmentation 5.36%, and segment-map insertion appeared among the main
hotspots. Raw profile: `/tmp/germinal-headless-unique-before.perf`.
Saved before binary: `/tmp/germinal-headless-before-segments`.

## Verification and limits

- Thirteen focused atlas tests passed. Cached collection is compared with fresh
  collection across scrolling/replacement, face changes, combining marks, emoji
  sequences, Arabic, both ligature modes, and preedit. Emptying the viewport
  releases cached runs while preserving preedit glyphs.
- Workspace format/check/tests and all-target/all-feature Clippy with warnings
  denied passed. Tests requiring host communication-runtime permissions ran
  outside the sandbox; no desktop benchmark ran.
- The earlier headless baseline and its raw data are preserved separately.
- CPU frequency and desktop background activity are uncontrolled. These are
  short CPU-stage measurements, not displayed FPS or end-to-end throughput.
- Visual window/compositor acceptance is pending, deliberately deferred to avoid
  interrupting the user's desktop. No claim of visual or GPU validation is made.
