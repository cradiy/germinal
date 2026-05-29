---
title: Germinal Domain Design
---

## Core Domain

### GShell

`GShell` is the core domain of Germinal.

A `GShell` is the main runtime unit of Germinal. It starts in `PtyHostMode`, switches to `GNativeMode` when a native structured application is requested, and returns to `PtyHostMode` after the native session exits.

```text
PtyHostMode -> GNativeMode -> PtyHostMode
```

---

## Supporting Domains

### Workspace

Workspace organizes where GShell instances appear.

It manages visible structure and focus but does not own runtime behavior.

---

### Rendering

Rendering defines what Germinal wants to draw.

It does not define how drawing is executed.

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
