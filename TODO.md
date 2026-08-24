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
- [x] Clear or preserve selections consistently when output, resize, focus, or viewport state
      changes.

### Vi copy mode

Vi mode here means a host-side copy and navigation mode. It must not change the shell's own input
editing mode.

- [x] Add a configurable shortcut to enter and leave Vi copy mode.
- [x] Render a visible Vi-mode cursor and mode indicator.
- [x] Support basic motions: `h`, `j`, `k`, `l`, `0`, `^`, `$`, `w`, `b`, `e`, `gg`, and `G`.
- [x] Support viewport motions: `Ctrl+U`, `Ctrl+D`, `Ctrl+B`, `Ctrl+F`, `H`, `M`, and `L`.
- [x] Support `/` and `?` search with `n` and `N` result navigation.
- [x] Support character-wise and line-wise visual selection with `v` and `V`.
- [x] Support Vim word motion and selection composition through `w`, `vw`, `viw`, and `vaw`.
- [x] Support `i` and `a` to leave Vi copy mode from navigation; use them as text-object prefixes
      in visual selection mode.
- [x] Support `y` to copy the active selection and return to Vi navigation mode.
- [x] Support `Escape` to cancel visual selection and pending text-object commands.
- [x] Support `q` to leave Vi copy mode.
- [x] Define behavior for new output while Vi copy mode is viewing older history.

## P0: IME and text input

- [x] Handle IME preedit updates in addition to committed text.
- [x] Render preedit text and its selection range at the active cursor.
- [x] Update the platform IME candidate area when the cursor or pane moves.
- [x] Keep independent IME composition state for PTY and GNative inputs.
- [x] Cancel composition safely on focus, mode, and pane changes.

## P1: Workspace operations

- [x] Expose runtime commands for horizontal and vertical pane splits.
- [x] Add shortcuts for creating, focusing, moving, resizing, and closing panes.
  - [x] Add configurable actions for creating, focusing, and closing panes.
  - [x] Add directional pane focus based on the visual layout.
  - [x] Add directional pane swapping with focus following the moved pane.
  - [x] Add directional pane resizing together with adjustable split ratios.
- [x] Add workspace tab creation, switching, reordering, and closing.
  - [x] Create, switch, and close tabs while inactive PTYs remain alive.
  - [x] Close a tab automatically when its final pane closes.
  - [x] Add tab reordering.
  - [x] Add a GPU-rendered title-only tab bar with configurable top/bottom placement.
- [x] Support adjustable split ratios instead of fixed half splits.
- [x] Keep runtime tabs ephemeral; do not persist or restore them across launches.
- [ ] Persist configurable startup layouts, split ratios, working directories, and startup commands.

## P1: Terminal compatibility

- [x] Support OSC 0 and OSC 2 dynamic window titles.
  - [x] Use OSC 0/2 metadata for live tab titles.
  - [x] Mirror the focused tab title into the native window title.
- [x] Support OSC 8 hyperlinks and pointer interaction.
- [x] Support OSC 52 clipboard operations with configurable security policy.
- [ ] Support audible and visual bell behavior.
  - [x] Add a configurable visual bell and request attention while the window is unfocused.
  - [x] Add an optional audible bell backend.
- [x] Add scrollback text search outside Vi copy mode.
- [x] Evaluate Kitty keyboard protocol support without changing GNative protocol semantics.
- [x] Add configurable cursor blinking and cursor styles.
- [x] Preserve combining marks, variation selectors, and ZWJ characters through PTY snapshots and
      render zero-width glyphs without advancing the terminal grid.
- [x] Add grapheme shaping for complex scripts and joined Emoji sequences, with outline-font
      fallback when the platform's color Emoji format cannot be rasterized.

## P1: Configuration

- [x] Configure the primary font family and font size.
- [x] Configure ordered fallback fonts and normal, bold, italic, and bold-italic faces.
- [ ] Configure OpenType font features.
- [x] Configure terminal, ANSI palette, cursor, selection, tab, border, and bell colors through
      Kitty-compatible theme files and inline overrides.
- [x] Configure terminal background opacity.
- [x] Configure the default shell, startup command, and working directory.
- [ ] Configure scrollback size and host-side shortcuts.
- [ ] Configure cursor behavior and workspace startup layout.
- [x] Validate configuration values and report actionable parse errors.

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

- [x] Define a separate experimental render-plugin API without changing the GNative protocol.
- [x] Provide a scoped render context around `wgpu::Device`, `wgpu::Queue`, command encoding, and
      the destination texture view.
- [ ] Isolate plugin failures and validate texture formats, sizes, usages, and submission order.
- [ ] Define resize, focus, damage, frame scheduling, and resource teardown callbacks.
- [ ] Evaluate an out-of-process shared-texture path using DMA-BUF and explicit synchronization.
- [x] Add a standalone wgpu plugin example before exposing the API as stable.

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
