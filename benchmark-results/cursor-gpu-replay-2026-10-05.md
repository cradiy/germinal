# Offscreen retained-frame replay: 2026-10-05

Base revision: `a2a86b6`, plus the working-tree regression and row-geometry guard.
Command: see `docs/terminal-offscreen-render-tests.md`.

Each run compared 432 RGBA8 frames (80 pixels wide, heights 63/64/65,
four cursor shapes, 36 frames per case). Full/reuse/damage/scroll-copy action
counts were 156/168/84/24. Every pixel matched fresh full rendering.

| Adapter | Backend | Driver | Result |
| --- | --- | --- | --- |
| llvmpipe, LLVM 22.1.8, 256 bits (CPU) | Vulkan | Mesa 26.2.3 | 432/432 |
| NVIDIA GeForce RTX 5060 Laptop GPU (discrete) | Vulkan | NVIDIA 615.71.09 | 432/432 |

The NVIDIA run used approved execution outside the sandbox. Both runs unset
`DISPLAY` and `WAYLAND_DISPLAY`, created no windows, and injected no input.
This is a correctness check, not a GPU performance benchmark.

## Detected regressions

Before the row-geometry guard, the replay failed on llvmpipe at height 63,
block cursor, frame 6, one-row scroll: pixel (1,15) was black instead of white.
Rounded row heights differed between the copy source and destination. A single
texture translation could not preserve those row boundaries. The retained
path now redraws fully when any copied boundary differs; geometrically valid
scroll copies remain enabled, including matching multirow fractional-height
patterns.

For sensitivity to the original cursor issue, the animation guard was
temporarily removed while retaining the geometry guard. The GPU replay failed
on llvmpipe at height 64, block cursor, frame 3, pixel (0,16): retained RGBA
was [212,212,212,255], full redraw was [255,255,255,255]. The animation guard
was restored afterward.

This validates the production draw/copy path with the debug glyph atlas.
System fonts, window presentation and compositor behavior remain untested.

## Repository checks

- `cargo fmt --all -- --check`: passed.
- `cargo check --offline --workspace --all-targets`: passed.
- `cargo test --offline --workspace --all-targets`: 621 passed, 2 ignored.
  The sandbox denied runtime creation in two existing local-session tests;
  the approved run outside the sandbox passed the full suite.
- `cargo clippy --offline --workspace --all-targets --all-features -- -D warnings`: passed.
- `git diff --check`: passed.
