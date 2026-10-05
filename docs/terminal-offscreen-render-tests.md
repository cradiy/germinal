# Offscreen retained-frame regression

Run the opt-in Vulkan pixel comparison without a display connection:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY cargo test --locked --offline \
    -p germinal-infra retained_gpu_pixels -- --ignored --nocapture
```

The test creates no window or presentation surface. It compares retained
rendering with a fresh full renderer using texture readback after each frame.
The same replay runs in RGBA8 and BGRA8, each with linear and sRGB attachment
formats. Raw pixel bytes are compared within each format, not across formats.
Both paths use the production terminal pipeline. The default is the debug glyph atlas.
The retained path uses the production damage selection and texture-copy code.
Terminal-store cases compose `cursor_snapshot` separately into the content
snapshot, matching the PTY worker. Content snapshots alone have no cursor.

To exercise the system-font rasterizer and shaping path instead, select a font:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY GERMINAL_GPU_REPLAY_FONT=monospace \
    cargo test --locked --offline -p germinal-infra retained_gpu_pixels -- --ignored --nocapture
```

The font option applies only to this ignored test. It uses 12-pixel glyphs in the
small test grid; font selection/fallback follows the host's font configuration.
Atlas initialization errors fail the test. The full-render reference initializes
an independent font atlas each frame, so this mode takes longer than debug glyphs.

The replay includes scrolling output, cursor animation and blinking, block,
underline, beam and hollow cursors, and uniform/nonuniform integer row heights.
An interleaved sequence supplies PTY output every four synthetic animation ticks
while cursor movement takes six ticks. It repeats scrolling output, colored text,
watch-style clears, shorter values, and cursor hiding/showing before settling
without further input. Every intermediate frame is read back and compared.
These supplied pixel positions test rendering under interrupted motion, not the
window runtime's animation clock, easing, or event scheduling.
It also feeds ANSI bytes through the terminal store for watch-style shorter
value updates, colored-row replacement and erasure, cursor moves/hiding,
display erasure, scrolling output, and alternate-screen entry/exit. Damage is
acknowledged between inputs, and the watch update must use partial repaint.
The padded-viewport case changes the default background with OSC 11 after
both retained textures have been used, then requires scrolling to reuse them.
Pixel comparison includes the top and bottom padding, not only terminal cells.
Stationary-hover cases supply fixed hyperlink highlights while identical labels
scroll beneath them. Partial damage must retain texture scrolling; damage covering
all content must skip copying and redraw fully. Both compare old/current highlight
pixels against the independent full-render path.
Scroll-region cases exercise upward/downward scrolling, reverse index, line
insertion/deletion, character insertion/deletion/erasure and region reset.
An eight-row viewport requires the region's text update to use partial repaint,
with pixels outside the damage area included in the full-frame comparison.
A resize sequence shrinks, expands and restores the terminal grid. It resizes
the terminal store and recreates targets using the presenter's size-matching
rule while keeping the retained renderer alive. Subsequent output must resume
scroll copies and match a freshly constructed full renderer.
Styled-text and Unicode cases erase bold/italic/underlined suffixes and replace
Chinese, emoji and combining characters with shorter text.
The eight-row case also calls the terminal's selection APIs to select, extend,
shrink, select a word/line and clear selection via a new empty selection. It
checks selected text as well as pixels, without mouse or clipboard integration.
History cases enter the oldest scrollback, move within history, receive output
while history remains visible, return to the live viewport and resume output.
They verify history offsets and cursor visibility while comparing the scrollbar
and all content pixels against full rendering.
Workspace composition cases reuse one renderer and target while rearranging two
panes horizontally/vertically, swapping target IDs and removing a pane. Background
alpha transitions through opaque, half-transparent, transparent and opaque again.
Each frame clears the target once, then draws panes with Load and no damage,
matching the non-retained terminal composition path. CPU assembly of independently
rendered pane textures is the reference; the comparison includes vacated regions
and gaps. A blank-cell check independently verifies the background alpha byte.
This does not exercise surface acquisition, retained-path eligibility, dividers,
plugins, background shaders, or the desktop compositor's transparency blending.
It requires all four retained actions (full, reuse, damage and scroll copy) to
execute. A missing Vulkan adapter is an error, not a skipped successful test.
The test prints the selected adapter; a CPU software adapter is not hardware
GPU validation. It is ignored in normal test runs so those need no GPU device.

This checks rasterized pixels and retained textures, not surface presentation,
compositor behavior, or real-window visual acceptance. The system-font mode
checks consistency, not the visual correctness or availability of each fallback glyph.
Record run results and adapter details under `benchmark-results/`.
