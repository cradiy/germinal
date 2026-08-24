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

## Building

Germinal provides Nushell and Bash build scripts. Run either command from the repository root:

```nu
./scripts/build-release.nu
```

```sh
./scripts/build-release.sh
```

The optimized executable is written to `target/product/germinal`.

Use `--target` to build for a specific Rust target:

```nu
./scripts/build-release.nu --target x86_64-unknown-linux-gnu
```

For Linux, `--libc gnu` and `--libc musl` select the corresponding target for the current
architecture:

```nu
./scripts/build-release.nu --libc musl
```

Targeted builds are written to `target/<rust-target>/product/germinal`.

## Linux packages

Create a portable Linux archive with:

```nu
./scripts/package-linux.nu
```

Release files and their SHA-256 checksums are written to `dist/`. Use `--format` to create a DEB,
an RPM, or every available package format:

```nu
./scripts/package-linux.nu --format deb
./scripts/package-linux.nu --format rpm
./scripts/package-linux.nu --format all
```

Use an existing product binary without rebuilding it:

```nu
./scripts/package-linux.nu --skip-build
```

Choose another output directory with `--output-dir <directory>`. DEB packaging requires
`dpkg-deb`, and RPM packaging requires `rpmbuild`. Packages include the executable, desktop entry,
application icon, license, and user documentation.

Bash users can replace the `.nu` suffix with `.sh`. Both scripts accept the same options.

### musl packages

Build and package a musl archive on a musl system or with a configured musl sysroot:

```nu
./scripts/build-release.nu --libc musl
./scripts/package-linux.nu --libc musl --skip-build
```

musl releases are distributed as portable archives. Germinal uses musl builds of Fontconfig,
FreeType, GLib, GStreamer, and the required GStreamer plugins, so these libraries must be available
on the target system. Cross-compilation also requires the Rust musl target, a matching C linker, and
a pkg-config sysroot.

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

## Keyboard shortcuts

Germinal consumes a key only when it matches a configured host binding. Every other key is sent to
the focused PTY or WGPU pane. Modifiers must match exactly, so a `Control|Shift` binding does not
also match `Control|Shift|Alt`.

### Default host bindings

| Shortcut | Action | Description |
| --- | --- | --- |
| `Ctrl+Shift+N` | `NewWindow` | Open an independent Germinal window in the focused shell's working directory. |
| `Ctrl+Shift+C` | `Copy` | Copy the active terminal selection. |
| `Ctrl+Shift+V` | `Paste` | Paste clipboard text using bracketed paste when the application enables it. |
| `Ctrl+Shift+Space` | `ToggleViMode` | Enter or leave host-side Vi copy mode. |
| `Ctrl+Shift+F` | `ToggleSearch` | Open or close host-side scrollback search. |
| `Ctrl+Shift+T` | `NewTab` | Create and focus a new tab. |
| `Ctrl+Shift+Left`, `Ctrl+Shift+H` | `PreviousTab` | Focus the previous tab. |
| `Ctrl+Shift+Right`, `Ctrl+Shift+L` | `NextTab` | Focus the next tab. |
| `Ctrl+Shift+Alt+H` | `MoveTabLeft` | Move the active tab left. |
| `Ctrl+Shift+Alt+L` | `MoveTabRight` | Move the active tab right. |
| `Ctrl+Shift+D` | `SplitHorizontal` | Split the focused pane into left and right panes. |
| `Ctrl+Shift+Alt+D` | `SplitVertical` | Split the focused pane into top and bottom panes. |
| `Ctrl+Shift+W` | `ClosePane` | Close the focused pane; closing its last pane closes the tab. |
| `Ctrl+Alt+Left` | `FocusPaneLeft` | Focus the pane to the left. |
| `Ctrl+Alt+Right` | `FocusPaneRight` | Focus the pane to the right. |
| `Ctrl+Alt+Up` | `FocusPaneUp` | Focus the pane above. |
| `Ctrl+Alt+Down` | `FocusPaneDown` | Focus the pane below. |
| `Ctrl+Shift+Alt+Left` | `SwapPaneLeft` | Swap the focused pane with the pane to the left. |
| `Ctrl+Shift+Alt+Right` | `SwapPaneRight` | Swap the focused pane with the pane to the right. |
| `Ctrl+Shift+Alt+Up` | `SwapPaneUp` | Swap the focused pane with the pane above. |
| `Ctrl+Shift+Alt+Down` | `SwapPaneDown` | Swap the focused pane with the pane below. |
| `Alt+Shift+Left` | `ResizePaneLeft` | Move the focused split toward the left. |
| `Alt+Shift+Right` | `ResizePaneRight` | Move the focused split toward the right. |
| `Alt+Shift+Up` | `ResizePaneUp` | Move the focused split upward. |
| `Alt+Shift+Down` | `ResizePaneDown` | Move the focused split downward. |

`FocusNextPane` and `FocusPreviousPane` are also available as configurable actions, but have no
default bindings. Directional focus, swap, and resize actions are passed to the PTY when the current
tab has only one pane. Tab-move actions are passed to the PTY when only one tab exists.

### Custom bindings

Bindings use an Alacritty-style array in `~/.config/germinal/config.toml`:

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

Defining `keyboard.bindings` replaces the complete default list. Include every default binding you
want to keep; remove the entire `[keyboard]` section to restore Germinal's defaults. An empty
binding list disables all host shortcuts.

`mods` accepts `Control`, `Alt`, `Shift`, and `Super`, joined with `|`. It may be omitted for an
unmodified key. Letter and digit keys use their printed names. Named keys are `Space`, `Enter`,
`Tab`, `Backspace`, `Escape`, `Left`, `Right`, `Up`, `Down`, `Home`, `End`, `Insert`, `Delete`,
`PageUp`, `PageDown`, `F1` through `F12`, `CapsLock`, `ScrollLock`, `NumLock`, `PrintScreen`,
`Pause`, and `ContextMenu`.

Available `action` values are:

- Clipboard and navigation: `Copy`, `Paste`, `ToggleViMode`, `ToggleSearch`.
- Windows and tabs: `NewWindow`, `NewTab`, `PreviousTab`, `NextTab`, `MoveTabLeft`,
  `MoveTabRight`.
- Pane creation and closing: `SplitHorizontal`, `SplitVertical`, `ClosePane`.
- Pane focus: `FocusNextPane`, `FocusPreviousPane`, `FocusPaneLeft`, `FocusPaneRight`,
  `FocusPaneUp`, `FocusPaneDown`.
- Pane swapping: `SwapPaneLeft`, `SwapPaneRight`, `SwapPaneUp`, `SwapPaneDown`.
- Pane resizing: `ResizePaneLeft`, `ResizePaneRight`, `ResizePaneUp`, `ResizePaneDown`.

### Host search keys

After opening host search with `Ctrl+Shift+F`:

| Key | Behavior |
| --- | --- |
| Text input | Update the search query. |
| `Enter` | Find the next match. |
| `Shift+Enter` | Find the previous match. |
| `Backspace` | Delete the previous query character. |
| `Escape` | Close host search. |

### Vi copy mode keys

Vi copy mode navigates and selects terminal history without changing the shell's own editing mode.

| Key | Behavior |
| --- | --- |
| `h`, `j`, `k`, `l` | Move left, down, up, or right. |
| `0`, `^`, `$` | Move to the first column, first occupied cell, or end of the line. |
| `w`, `b`, `e` | Move by words. |
| `gg`, `G` | Move to the top or bottom of history. |
| `H`, `M`, `L` | Move to the top, middle, or bottom of the viewport. |
| `Ctrl+U`, `Ctrl+D` | Move half a page up or down. |
| `Ctrl+B`, `Ctrl+F` | Move one page up or down. |
| `v`, `V` | Toggle character-wise or line-wise visual selection. |
| `viw`, `vaw` | Select the inner word or the word including surrounding separators. |
| `y` | Copy the visual selection and return to Vi navigation. |
| `/`, `?` | Start a forward or backward search. |
| `n`, `N` | Repeat the last search in the same or opposite direction. |
| `i`, `a`, `q` | Leave Vi mode when no visual selection is active. |
| `Escape` | Cancel the visual selection or pending command without leaving Vi mode. |

While entering a Vi search, text updates the query, `Backspace` deletes a character, `Enter`
accepts it, and `Escape` cancels the prompt.

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
