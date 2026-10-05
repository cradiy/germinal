# Offscreen retained-frame regression

Run the opt-in Vulkan pixel comparison without a display connection:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY cargo test --locked --offline \
    -p germinal-infra retained_gpu_pixels -- --ignored --nocapture
```

The test creates no window or presentation surface. It compares retained
rendering with a fresh full renderer using RGBA8 texture readback after each
frame. Both paths use the production terminal pipeline and debug glyph atlas.
The retained path uses the production damage selection and texture-copy code.

The replay includes scrolling output, cursor animation and blinking, block,
underline, beam and hollow cursors, and uniform/nonuniform integer row heights.
It requires all four retained actions (full, reuse, damage and scroll copy) to
execute. A missing Vulkan adapter is an error, not a skipped successful test.
The test prints the selected adapter; a CPU software adapter is not hardware
GPU validation. It is ignored in normal test runs so those need no GPU device.

This checks rasterized pixels and retained textures, not system-font rendering,
surface presentation, compositor behavior, or real-window visual acceptance.
Record run results and adapter details under `benchmark-results/`.
