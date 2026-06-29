# Germinal

Germinal is a keyboard-first graphical terminal that combines PTY shell compatibility with structured UI apps.

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
