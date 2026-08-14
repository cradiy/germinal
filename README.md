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

## PTY input modes

Germinal tracks terminal modes emitted by PTY applications. Application cursor keys, bracketed paste, focus reporting, and SGR mouse click, drag, motion, and wheel events are encoded according to the active mode.

PTY keyboard input includes xterm-compatible navigation and editing keys, `F1` through `F12`, `Shift+Tab`, and modifier parameters for cursor, editing, and function keys.
