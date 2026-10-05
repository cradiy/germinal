# Clipboard lifetime regression

The X11 lifetime test writes both CLIPBOARD and PRIMARY. Run it only on a
dedicated X server without a clipboard manager, never on the desktop display.
It is ignored by normal test runs and requires an explicit display opt-in.

With Xvfb and `xvfb-run` installed:

```sh
xvfb-run -a sh -c '
  export GERMINAL_TEST_X11_DISPLAY="$DISPLAY"
  unset WAYLAND_DISPLAY GERMINAL_TEST_CLIPBOARD_READER
  cargo test --locked --offline -p germinal --lib \
    x11_copy_survives_return_focus_reset_and_replacement -- --ignored --nocapture
'
```

The application controller writes Unicode text to both selections, resets its
input modifiers as on focus loss, then starts a separate reader process. It
repeats the read after replacing the contents. The writer remains alive without
relying on a clipboard manager to preserve the selection.

This covers X11 ownership, including the clipboard backend used when falling
back from Wayland to X11. It does not validate compositor bridging, native
Wayland data-control support, shortcut delivery, or persistence after app exit.
