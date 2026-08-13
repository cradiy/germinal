---
title: Germinal Domain Design
---

## Core Domain

### GShell

`GShell` is the core domain of Germinal.

A `GShell` is the main runtime unit of Germinal. It starts in `PtyHostMode`, enters a non-blocking connecting state when a native structured application is requested, switches to `GNativeMode` after the tunnel accepts the session, and returns to `PtyHostMode` after the native session exits or connection fails.

```text
PtyHostMode -> GNativeConnectingMode -> GNativeMode -> PtyHostMode
                         \-> connection failed -> PtyHostMode
```

---

## Supporting Domains

### Workspace

Workspace organizes where GShell instances appear.

It owns the pane tree, visible structure, split direction, and focused `PaneId`, but does not own GShell runtime behavior. The application layer binds visible panes to GShell instances and turns the pane tree into pixel placements for rendering.

---

### Rendering

Rendering defines what Germinal wants to draw.

It does not define how drawing is executed. A window frame may contain multiple render targets; infrastructure composes them into one swapchain image using target-specific viewport and scissor rectangles.

---

## External Capabilities

The following are not domain concepts:

- PTY
- ConPTY
- winit
- wgpu
- glyphon
- alacritty_terminal
- OS windows
- GPU devices
- terminal parser implementations

These capabilities are exposed through ports and implemented in infrastructure.

---

## Cross-Domain Rule

Domains do not directly depend on each other.

```text
Workspace owns PaneId.
GShell owns GShellId.
Application owns PaneId -> GShellId binding.
```

`Pane` must not store `GShellId`.

---

## Application Layer

The application layer composes domains.

It is responsible for:

- creating Workspace
- creating GShell
- binding `PaneId` to `GShellId`
- routing input
- reacting to runtime mode changes
- coordinating runtime effects
- invoking external ports
