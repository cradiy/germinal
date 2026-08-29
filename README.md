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

## Design documents

- [GNative over SSH](docs/gnative-over-ssh.md) describes the planned remote rendering transport,
  protocol negotiation, OpenSSH integration, security boundaries, and implementation phases.

## Configuration

See [Configuration](docs/configuration.md) for the config file location, defaults, and every
supported option.

## Building

Germinal provides Nushell build scripts. Run the command from the repository root:

```nu
./scripts/build-release.nu
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
an RPM, an Arch Linux package, or every available package format:

```nu
./scripts/package-linux.nu --format deb
./scripts/package-linux.nu --format rpm
./scripts/package-linux.nu --format arch
./scripts/package-linux.nu --format all
```

Use an existing product binary without rebuilding it:

```nu
./scripts/package-linux.nu --skip-build
```

Choose another output directory with `--output-dir <directory>`. DEB packaging requires
`dpkg-deb`, RPM packaging requires `rpmbuild`, and Arch Linux packaging requires `makepkg`.
Packages include the executable, desktop entry, application icon, license, and user documentation.

### musl packages

Build and package a musl archive on a musl system or with a configured musl sysroot:

```nu
./scripts/build-release.nu --libc musl
./scripts/package-linux.nu --libc musl --skip-build
```

musl releases are distributed as portable archives. Germinal uses musl builds of Fontconfig and
FreeType, so these libraries must be available on the target system. Cross-compilation also requires
the Rust musl target, a matching C linker, and a pkg-config sysroot.

## Terminal images

Germinal supports Kitty Graphics Protocol images using raw RGB, raw RGBA, or PNG payloads. Images can be transferred directly, through regular or temporary files, or through POSIX shared memory. Chunked Base64 payloads, zlib compression, image and placement IDs, Unicode placeholders used by Yazi and Neovim, source rectangles, cell sizing, protocol z-index layering, deletion, query responses, alternate screens, scrolling-margin clipping, and client- or terminal-driven animation are supported. Animation support includes partial frame updates, frame timing and loops, frame composition, and animation-frame deletion.

To render the standalone checkerboard example, run this command inside Germinal:

```
cargo run -p germinal --example kitty_image
```

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
