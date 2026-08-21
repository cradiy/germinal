# Germinal

Germinal is a keyboard-first graphical terminal that combines PTY shell compatibility with structured UI apps.

## Project origin

This project originated as a fork of [zooeywm/germinal](https://github.com/zooeywm/germinal).

## Demo

run

```
cargo run -r -p germinal
```

in germinal, cd to this project and run

```
cargo run -r -p germinal-gnative-demo
```

The germinal first start up with `PTY` mode (using alacritty-terminal for parsing), when the `germinal-gnative-demo` starts, the germinal enters into `gnative` mode, which is a actually GUI.

## Terminal images

Germinal supports direct Kitty Graphics Protocol images using raw RGB, raw RGBA, or PNG payloads. Chunked base64 payloads, zlib compression, image/placement IDs, Unicode placeholders used by Yazi, source rectangles, cell sizing, z-index layering, deletion, and query responses are supported.

To render the standalone checkerboard example, run this command inside Germinal:

```
cargo run -p germinal --example kitty_image
```

File, temporary-file, shared-memory, and animation actions are not supported yet. Unsupported requests receive a protocol error when an image ID is provided.

## Desktop notifications

Terminal applications can request native system notifications through Kitty OSC 99 or the legacy
OSC 9 sequence. For example:

```sh
printf '\e]9;Build finished\a'
printf '\e]99;i=build:d=0;Cargo\e\\'
printf '\e]99;i=build:p=body;Tests passed\e\\'
```

OSC 99 title and body payloads, chunking, Base64 encoding, visibility conditions, and capability
queries are supported. Notifications are delivered asynchronously so a slow desktop notification
service does not block terminal rendering. Activating a notification switches to and focuses its
source tab and panel when they are still open.

## PTY input modes

Germinal tracks terminal modes emitted by PTY applications. Application cursor keys, bracketed paste, focus reporting, and SGR mouse click, drag, motion, and wheel events are encoded according to the active mode.

PTY keyboard input includes xterm-compatible navigation and editing keys, `F1` through `F12`, `Shift+Tab`, and modifier parameters for cursor, editing, and function keys.

## Fonts

The normal face, styled faces, fallback order, and size are configured together:

```toml
[font]
size = 16
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

Styled faces are optional. When omitted, Germinal asks the primary family for the matching
weight and slant. Fallback families are checked in their configured order for glyphs missing from
the selected primary face; the system font fallback remains the final fallback.

## Background opacity

Window background opacity is configured independently from the terminal colors:

```toml
[window]
opacity = 0.92
```

The value must be between `0.0` (fully transparent background) and `1.0` (opaque). Text, images,
cursor content, and explicit application background colors remain opaque. Transparency also
depends on support from the window system and compositor.

## Shader backgrounds

Germinal includes an animated meteor-shower starfield background. Enable it in
`~/.config/germinal/config.toml`:

```toml
[background]
shader = "starfield"
```

You can also load a custom WGSL file. Relative paths are resolved from
`~/.config/germinal`, and paths beginning with `~` are expanded:

```toml
[background]
shader = "shaders/background.wgsl"
animated = true
```

The WGSL file defines this function:

```wgsl
fn background(
    uv: vec2<f32>,
    time: f32,
    resolution: vec2<f32>,
) -> vec4<f32> {
    return vec4<f32>(uv.x, uv.y, 0.2, 1.0);
}
```

`uv` runs from the top-left corner `(0, 0)` to the bottom-right corner `(1, 1)`,
`time` is the elapsed time in seconds, and `resolution` is the window size in physical pixels.
Custom shaders are static by default; set `animated = true` when the shader uses `time`. The
built-in starfield is animated automatically. `window.opacity` is applied to the shader output.

## Kitty color themes

Germinal can load a Kitty color theme directly. Set one path in
`~/.config/germinal/config.toml` to switch the entire terminal theme:

```toml
[colors]
theme = "~/.config/kitty/current-theme.conf"
```

The theme uses Kitty's plain-text color syntax, including `foreground`, `background`,
`cursor`, selection colors, tab and border colors, and `color0` through `color255`.
Paths beginning with `~` are expanded; relative paths are resolved from
`~/.config/germinal`. Individual Kitty keys can override the selected file without
copying the theme:

```toml
[colors]
theme = "themes/Tokyo Night.conf"
cursor = "#ffffff"
active_tab_background = "#7aa2f7"
```

Built-in colors are applied first, followed by the theme file and then the inline
overrides. Theme colors currently accept Kitty's `#RGB` and `#RRGGBB` forms.

## Shell and working directory

The default shell command and initial working directory can be configured in
`~/.config/germinal/config.toml`:

```toml
[terminal]
working_directory = "~/github"

[terminal.shell]
program = "/usr/bin/fish"
args = ["--login"]
```

On Unix, omitting `terminal.shell` uses `$SHELL` and falls back to `/bin/sh`; Windows keeps its
PowerShell default. Without `working_directory`, the initial shell inherits Germinal's process
directory. On Linux, newly created tabs and panes inherit the focused shell's live working
directory.
