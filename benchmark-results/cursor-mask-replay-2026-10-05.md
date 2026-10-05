# Retained cursor mask replay: 2026-10-05

Runtime code: `a2a86b6`. Added CPU-only regression:
`retained_cursor_mask_matches_full_redraw_over_scroll_and_animation_sequences`.

The replay models a single block-cursor column, applies retained-frame scroll
copies and scissored repaint ranges, and compares each result with a full redraw
mask. It executes 180 frames for each combination of content heights 10, 63, 64,
and 65 pixels and vertical origins 0 and 3: 1,440 compared frames in total.
It combines output scrolling, intermediate animation positions, settled cursor
positions, blink visibility, and nonuniform integer row heights.

With the scroll/animation guard in place, every compared frame passes. As a
sensitivity check, the guard was temporarily removed and the test rerun. It
failed at height 10, origin 0, frame 3: a one-row scroll repainted pixels 5..10
but left stale cursor pixels at vertical coordinates 3 and 4. Full redraw had
only the live cursor at pixels 7..10. The guard was restored afterward.

```sh
cargo test --offline -p germinal-infra retained_cursor_mask_matches
```

This is a deterministic CPU coverage model, not an offscreen GPU renderer. It
does not validate actual texture formats, GPU copies, text blending, horizontal
clipping, other cursor shapes, presentation, or compositor behavior. No window,
GPU device, or desktop input was used. Real-window visual acceptance remains
pending. No production rendering code was changed in this follow-up.
