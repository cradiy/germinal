# Headless terminal CPU benchmark

Build the example in release mode:

```sh
cargo build --locked --release -p germinal --example terminal_headless_benchmark
```

Run without a display connection:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile unicode-repeated --frames 5000 --repeats 5
```

The benchmark creates neither windows nor GPU devices. It feeds an in-process
terminal store and prepares CPU glyph atlases using the installed fonts. It
does not launch a shell, read the user configuration, or inject keyboard input.
Font discovery and rasterization still require a working system font backend.

Profiles are `ascii`, `unicode-repeated`, `unicode-unique`, and `links` (plain URLs
and OSC 8). Unique Unicode lines reorder a fixed character vocabulary so glyph
cache growth does not dominate the measurement. All profiles scroll colored
lines with changing numeric prefixes through a default 142x40 grid.

`--lines-per-frame` controls input batching; `--columns` and `--rows` control
geometry. `--font` selects the family at size 20. Ligatures are disabled unless
`--ligatures` is supplied. Each repetition creates a fresh terminal and atlas,
uses 10,000 scrollback lines, and excludes `--warmup` frames (default 100).
Use enough warm-up frames to fill the selected grid and warm its glyphs.

CSV output reports mean microseconds per measured frame for:

- `parse_us`: `apply_bytes`, including protocol decoding and terminal mutation;
- `snapshot_us`: styled render-surface snapshot generation;
- `links_us`: visible-link discovery, including disposal of its result;
- `atlas_us`: CPU glyph-atlas frame preparation, including collection and cache
  lookup, but no GPU upload.

`atlas_hits` counts reusable glyph-atlas frames; `content_hits` counts the separate
unchanged-sequence shortcut. Every frame changes the sequence, so content hits
should remain zero. Payload generation, font initialization, damage clearing,
and final snapshot/atlas-frame disposal are outside the timed stages. The corpus
contains 4,096 lines and repeats cyclically. `bytes` is measured input size.

This is a CPU-stage diagnostic, not end-to-end terminal throughput or a Kitty
comparison. It excludes PTY transport, worker scheduling, GPU rendering,
presentation, input latency, and compositor behavior. Do not interpret stage
times as displayed FPS. Avoid concurrent builds or other benchmarks when
collecting results; retain individual CSV rows alongside the environment and
revision in `benchmark-results/` when recording an experiment.
