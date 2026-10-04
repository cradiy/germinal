# Niri / NVIDIA bare-terminal comparison

`scripts/terminal-compare.py` launches isolated Germinal and Kitty windows on the current Niri
desktop. It requires Python 3, Niri, NVIDIA `nvidia-smi`, Kitty, and Fontconfig. `grim` enables
screenshots during warm-up runs. Existing application configuration is not modified.

Build before sampling:

```sh
cargo build --locked --release -p germinal --bin germinal --example terminal_benchmark
python3 scripts/terminal-compare.py run --output /tmp/terminal-comparison-unique
```

Run from the desktop session with `XDG_RUNTIME_DIR`, `WAYLAND_DISPLAY`, `DBUS_SESSION_BUS_ADDRESS`,
and `NIRI_SOCKET` set. The script temporarily opens fullscreen test windows; leave them focused
until completion. Each child PTY runs its own workload without injecting desktop keyboard input.
Output directories must not already exist.

If a run stops, repeat the same command with `--resume`. Completed cases are retained and skipped;
failed attempts are kept in their original directories and retried under a new directory name.
Resume requires matching parameters, Git revision, and display configuration.

The default font is JetBrainsMono Nerd Font at Germinal size 20 and Kitty size 15 points. Both
windows must report identical compositor dimensions and PTY rows/columns. A mismatch stops the
run; use `--font-size` and `--kitty-font-size` to calibrate. `--repeats 0 --workloads idle` performs
only a calibration/warm-up pass. The script checks focus and size before and after each run.
Both terminals use 10,000 scrollback lines, opaque backgrounds, disabled ligatures, and disabled
cursor blinking/motion animation.

The default run performs one warm-up plus five measured repetitions for each workload, alternating
terminal order between repetitions:

- idle with a hidden cursor;
- deterministic table clearing/redrawing every 200 ms (`watch-clear`);
- ASCII and Unicode paced output at `--hz` (default 165);
- ASCII and Unicode floods of `--lines` (default 250000);
- cursor position changes every 30 ms, with cursor animations disabled;
- static RGBA and PNG images;
- Kitty graphics animations requesting 8 ms and 16 ms frame intervals.

`watch-clear` is a deterministic clear/redraw workload, not an invocation of procps `watch` or
`nvidia-smi`. Cursor motion measures output-driven positioning, not keyboard-to-display latency.
Use `--seconds` to set timed workload duration (default 10), and `--workloads` to select cases.
The scope is bare terminals; it does not implement the Zellij matrix described in
[the full comparison specification](performance-comparison.md).

Each case retains CPU/RSS samples, raw NVIDIA process counters, board measurements, launch logs,
PTY geometry, producer results, and a JSON summary. Warm-ups also capture screenshots when `grim`
is available. The root `summary.md` reports medians across measured repetitions. `environment.json`
records the revision, dirty state, display modes, NVIDIA state, font file, and parameters.

CPU percentages use one CPU core as 100%. CPU totals include the workload and three seconds after
producer completion. RSS is resident memory, not private/PSS memory. GPU SM utilization and VRAM
come from continuous `nvidia-smi pmon` samples for the terminal PID. Missing counters remain null;
they are not treated as zero. GPU utilization and power queried at board level include other
desktop processes. Image upload and playback costs are combined in the aggregate table; raw
samples retain their time series. Producer completion and requested Hz do not measure displayed
frame rate or presentation latency. Screenshots are taken only in excluded warm-up runs and must
be reviewed before treating a workload as visually valid.

CPU sampling begins after detecting the start marker (20 ms polling granularity), so very short
transfers can precede its first sample. Use producer transmission logs for delivery time; do not
use the aggregate CPU number as a precise image-upload cost. Process SM averages use only available
samples; inspect missing-counter coverage before comparing them.
