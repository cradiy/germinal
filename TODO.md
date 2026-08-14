# Germinal TODO

This document tracks the remaining work required to make Germinal a practical terminal and a
stable platform for GNative applications.

## Working rules

- Do not change the GNative protocol version unless a protocol upgrade is explicitly requested.
- Keep experimental functionality in separate examples until its public API is stable.
- Preserve PTY application behavior when adding host-side shortcuts or pointer handling.
- Validate terminal-facing changes with real applications such as Neovim and Yazi.

## P0: Scrollback, selection, and Vi copy mode

- [x] Use a bounded scrollback buffer for every PTY pane.
- [x] Make the scrollback history limit configurable.
- [x] Track a per-pane viewport offset independently from the terminal cursor.
- [x] Route mouse-wheel input to scrollback when the PTY application has not enabled mouse
      reporting.
- [x] Keep alternate-screen applications such as Neovim and Yazi isolated from normal-screen
      scrollback.
- [x] Add character, word, and line selection with mouse dragging, double-click, and triple-click.
- [x] Add `Ctrl+Shift+C` clipboard copy without interfering with PTY input.
- [ ] Clear or preserve selections consistently when output, resize, focus, or viewport state
      changes.

### Vi copy mode

Vi mode here means a host-side copy and navigation mode. It must not change the shell's own input
editing mode.

- [ ] Add a configurable shortcut to enter and leave Vi copy mode.
- [ ] Render a visible Vi-mode cursor and mode indicator.
- [ ] Support basic motions: `h`, `j`, `k`, `l`, `0`, `^`, `$`, `w`, `b`, `e`, `gg`, and `G`.
- [ ] Support viewport motions: `Ctrl+U`, `Ctrl+D`, `Ctrl+B`, `Ctrl+F`, `H`, `M`, and `L`.
- [ ] Support `/` and `?` search with `n` and `N` result navigation.
- [ ] Support character-wise and line-wise visual selection with `v` and `V`.
- [ ] Support `y` to copy the active selection to the system clipboard.
- [ ] Support `Escape` and `q` to cancel selection or leave Vi copy mode.
- [ ] Define behavior for new output while Vi copy mode is viewing older history.

## P0: IME and text input

- [ ] Handle IME preedit updates in addition to committed text.
- [ ] Render preedit text and its selection range at the active cursor.
- [ ] Update the platform IME candidate area when the cursor or pane moves.
- [ ] Keep independent IME composition state for PTY and GNative inputs.
- [ ] Cancel composition safely on focus, mode, and pane changes.

## P1: Workspace operations

- [ ] Expose runtime commands for horizontal and vertical pane splits.
- [ ] Add shortcuts for creating, focusing, moving, resizing, and closing panes.
- [ ] Add workspace tab creation, switching, reordering, and closing.
- [ ] Support adjustable split ratios instead of fixed half splits.
- [ ] Persist tabs, pane trees, split ratios, working directories, and startup commands.
- [ ] Restore sessions without leaking stale PTY, GNative, media, or GPU resources.

## P1: Terminal compatibility

- [ ] Support OSC 0 and OSC 2 dynamic window titles.
- [ ] Support OSC 8 hyperlinks and pointer interaction.
- [ ] Support OSC 52 clipboard operations with configurable security policy.
- [ ] Support audible and visual bell behavior.
- [ ] Add scrollback text search outside Vi copy mode.
- [ ] Evaluate Kitty keyboard protocol support without changing GNative protocol semantics.
- [ ] Add configurable cursor blinking and cursor styles.

## P1: Configuration

- [x] Configure the primary font family and font size.
- [ ] Configure fallback fonts, font weight, and font features.
- [ ] Configure terminal colors, opacity, cursor colors, and selection colors.
- [ ] Configure the default shell, startup command, and working directory.
- [ ] Configure scrollback size and host-side shortcuts.
- [ ] Configure cursor behavior and workspace startup layout.
- [ ] Validate configuration values and report actionable parse errors.

## P2: GNative interaction model

- [ ] Add stable element identifiers to the GNative UI tree.
- [ ] Add host-side hit testing and targeted pointer event dispatch.
- [ ] Add hover, pressed, focus, blur, click, and keyboard activation semantics.
- [ ] Add focus traversal and accessibility-oriented element metadata.
- [ ] Make buttons, checkboxes, and inputs interactive without demo-specific event routing.
- [ ] Define event ordering and stale-event handling across frame updates.

## P2: GNative rendering primitives

- [ ] Implement real image elements instead of `div()` placeholders.
- [ ] Implement canvas/custom-paint surfaces with clipping.
- [ ] Implement SVG rendering.
- [ ] Implement scroll containers and virtualized lists.
- [ ] Add transforms, stacking order, clipping, and nested opacity.
- [ ] Define resource creation, reuse, invalidation, and release semantics.

## P3: Third-party wgpu rendering

- [ ] Define a separate experimental render-plugin API without changing the GNative protocol.
- [ ] Provide a scoped render context around `wgpu::Device`, `wgpu::Queue`, command encoding, and
      the destination texture view.
- [ ] Isolate plugin failures and validate texture formats, sizes, usages, and submission order.
- [ ] Define resize, focus, damage, frame scheduling, and resource teardown callbacks.
- [ ] Evaluate an out-of-process shared-texture path using DMA-BUF and explicit synchronization.
- [ ] Add a standalone wgpu plugin example before exposing the API as stable.

## P3: Kitty Graphics Protocol completion

- [ ] Support file and temporary-file transmission media with explicit security constraints.
- [ ] Support shared-memory transmission where the platform permits it.
- [ ] Support animation frames and animation control actions.
- [ ] Complete deletion selectors and image-number semantics.
- [ ] Add compatibility tests for Kitty, Yazi, and other image-producing terminal applications.

## Documentation and release quality

- [ ] Replace the default Starlight documentation content with Germinal documentation.
- [ ] Document installation, configuration, shortcuts, image support, and troubleshooting.
- [ ] Document the GNative SDK lifecycle and provide minimal application examples.
- [ ] Add CI for formatting, workspace checks, tests, and supported platforms.
- [ ] Add release packaging and a compatibility test matrix.
