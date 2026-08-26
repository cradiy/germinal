# Configuration

Germinal reads `~/.config/germinal/config.toml`. If the file does not exist, Germinal creates it
with the current defaults. When `XDG_CONFIG_HOME` is set, the path is
`$XDG_CONFIG_HOME/germinal/config.toml`.

All sections and fields are optional. Omitted values use their defaults.

## Window

```toml
[window]
title = "Germinal"
width_px = 960
height_px = 540
maximized = false
opacity = 1.0
decorations = true
```

- `width_px` and `height_px` are integer logical-pixel values. Use `960`, not `960px`.
- `maximized = true` starts each Germinal window maximized while keeping the desktop shell and
  compositor window-management behavior. The default is `false`.
- `opacity` accepts values from `0.0` through `1.0`. Compositor support is required for
  transparency.
- `decorations = false` requests a frameless window. The compositor may still draw a focus border.

## Font

```toml
[font]
size = 16.0
ligatures = true
fallback = [
  "Noto Sans Mono CJK SC",
  "Symbols Nerd Font Mono",
]

[font.normal]
family = "JetBrainsMono Nerd Font"

[font.bold]
family = "JetBrainsMono Nerd Font"
style = "Bold"

[font.italic]
family = "JetBrainsMono Nerd Font"
style = "Italic"

[font.bold_italic]
family = "JetBrainsMono Nerd Font"
style = "Bold Italic"
```

The platform default normal family is `monospace` on Linux, `Menlo` on macOS, and `Consolas` on
Windows. Styled faces and `fallback` are optional. Omitted styled faces are resolved from the
normal family. Fallbacks are tried in order before system fallback.

Font size follows the display scale. Set `ligatures = false` to disable programming and standard
font ligatures.

## Cursor

```toml
[cursor]
shape = "block"
blinking = false
blink_interval_ms = 750
motion_duration_ms = 80
motion_on_input = true
motion_on_enter = true
```

- `shape` accepts `block`, `underline`, or `beam`.
- `motion_on_input` and `motion_on_enter` independently control the two cursor animations.
- `motion_duration_ms = 0` disables all cursor motion animation.

## Colors

```toml
[colors]
theme = "~/.config/kitty/current-theme.conf"
foreground = "#cdd6f4"
background = "#1e1e2e"
cursor = "#f5e0dc"
active_tab_background = "#89b4fa"
```

`theme` loads a Kitty color-theme file. Paths beginning with `~` are expanded; relative paths are
resolved from the Germinal config directory. Inline values override the theme file, which overrides
the built-in colors. Colors accept `#RGB` and `#RRGGBB`.

Supported inline keys are `foreground`, `background`, `cursor`, `cursor_text_color`,
`selection_foreground`, `selection_background`, `url_color`, `active_border_color`,
`inactive_border_color`, `bell_border_color`, `tab_bar_background`, `active_tab_foreground`,
`active_tab_background`, `inactive_tab_foreground`, `inactive_tab_background`, and `color0`
through `color255`.

`cursor` also accepts `none`; `cursor_text_color` accepts `background` or `none`; selection colors
accept `none`.

## Background shader

Use the built-in animated background:

```toml
[background]
shader = "starfield"
```

Or load a custom WGSL file:

```toml
[background]
shader = "shaders/background.wgsl"
animated = true
```

Relative paths are resolved from the config directory and `~` is expanded. Custom shaders are
static by default; set `animated = true` when the shader uses time. A custom shader must define:

```wgsl
fn background(
    uv: vec2<f32>,
    time: f32,
    resolution: vec2<f32>,
) -> vec4<f32>
```

`window.opacity` is applied to the shader output.

## Terminal

```toml
[terminal]
osc52 = "OnlyCopy"
working_directory = "~/github"

[terminal.shell]
program = "/usr/bin/fish"
args = ["--login"]
```

`osc52` accepts `Disabled`, `OnlyCopy`, `OnlyPaste`, or `CopyPaste`. The default is `OnlyCopy`.

`working_directory` and `terminal.shell` are optional. On Unix, omitting the shell uses `$SHELL`
and falls back to `/bin/sh`. Without `working_directory`, the first shell inherits Germinal's
process directory. On Linux, new tabs and panes inherit the focused shell's current directory.

## Scrolling

```toml
[scrolling]
history = 10000
```

`history` is the maximum scrollback line count.

## Bell

```toml
[bell]
duration_ms = 150
urgent_on_unfocused = true

[bell.command]
program = "canberra-gtk-play"
args = ["--id", "bell"]
```

`bell.command` is optional. `duration_ms` controls the visual bell duration.

## Tabs

```toml
[tabs]
position = "bottom"
```

`position` accepts `top` or `bottom`.

## Keyboard

```toml
[[keyboard.bindings]]
key = "H"
mods = "Control|Shift"
action = "PreviousTab"

[[keyboard.bindings]]
key = "L"
mods = "Control|Shift"
action = "NextTab"
```

To disable the default bindings and use only explicitly configured bindings:

```toml
[keyboard]
use_default_bindings = false

[[keyboard.bindings]]
key = "V"
mods = "Control|Shift"
action = "ToggleViMode"
```

Setting `use_default_bindings = false` without any `keyboard.bindings` disables all host shortcuts.

`mods` accepts `Control`, `Alt`, `Shift`, and `Super`, joined with `|`. Omit it for an unmodified
key. Letter and digit keys use their printed names. Named keys are `Space`, `Enter`, `Tab`,
`Backspace`, `Escape`, `Left`, `Right`, `Up`, `Down`, `Home`, `End`, `Insert`, `Delete`, `PageUp`,
`PageDown`, `F1` through `F12`, `CapsLock`, `ScrollLock`, `NumLock`, `PrintScreen`, `Pause`, and
`ContextMenu`.

Available actions:

- `Copy`, `Paste`, `ToggleViMode`, `ToggleSearch`
- `NewWindow`, `NewTab`, `PreviousTab`, `NextTab`, `MoveTabLeft`, `MoveTabRight`
- `SplitHorizontal`, `SplitVertical`, `ClosePane`
- `FocusNextPane`, `FocusPreviousPane`, `FocusPaneLeft`, `FocusPaneRight`, `FocusPaneUp`,
  `FocusPaneDown`
- `SwapPaneLeft`, `SwapPaneRight`, `SwapPaneUp`, `SwapPaneDown`
- `ResizePaneLeft`, `ResizePaneRight`, `ResizePaneUp`, `ResizePaneDown`

## Logging

```toml
[logging]
console_level = "debug"
file_level = "info"
```

Both fields accept `trace`, `debug`, `info`, `warn`, or `error`.
