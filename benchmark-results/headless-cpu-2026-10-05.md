# Headless CPU baseline: 2026-10-05

Measured release code at `39c3890` with the new `terminal_headless_benchmark`
example. CPU: AMD Ryzen AI 7 H 350, 16 logical CPUs. Rust: 1.98.0.
Font: JetBrainsMono Nerd Font size 20; ligatures disabled; 142x40 grid;
10,000 scrollback lines. No production implementation changes follow that commit.

Each profile ran sequentially, with five fresh instances, 100 warm-up frames,
and 5,000 measured frames per instance. Payload size and content differ by
profile; these are diagnostic workloads, not matched text-throughput comparisons.
DISPLAY and WAYLAND_DISPLAY were removed from the benchmark environment.
No windows, GPU devices, PTY subprocesses, or desktop input were used.

Command, repeated for each profile:

```sh
env -u DISPLAY -u WAYLAND_DISPLAY target/release/examples/terminal_headless_benchmark \
    --profile unicode-repeated --frames 5000 --repeats 5
```

Median of five per-run means, in microseconds per frame:

| Profile | Parse | Snapshot | Links | Atlas preparation |
|---|---:|---:|---:|---:|
| UnicodeRepeated | 1.268 | 48.077 | 25.257 | 26.958 |
| UnicodeUnique | 1.589 | 42.907 | 23.861 | 99.818 |
| Ascii | 1.324 | 50.849 | 25.786 | 0.505 |
| Links | 1.028 | 50.681 | 46.807 | 0.609 |

All measured frames hit the glyph atlas cache; none hit the unchanged-content
shortcut. Different sequence numbers ensure each frame exercises collection.
Raw repetitions are retained in [the CSV](headless-cpu-2026-10-05.csv), including
the slower first ASCII repetition.

The changing-order Unicode workload still spends most of its measured time in
atlas preparation despite raster-cache hits. Repeated Unicode avoids much of
that work, making snapshot generation the largest measured stage. Link discovery
also scans the visible grid even in profiles without links. These are priorities
for focused profiling, not proof that a specific replacement will be faster.

Limits: no PTY transport, worker scheduling, GPU upload/render, compositor,
displayed FPS, or input latency is measured. Cold font initialization and fixture
generation are excluded. CPU frequency is not pinned, and desktop background
activity is uncontrolled. Each stage is timed separately; their sum omits damage
clearing and final snapshot/atlas-frame disposal. Font/backend differences can
change results. This is not directly comparable to the desktop CPU percentages
or Kitty measurements.

Validation: release execution of all four profiles with display variables
removed; zero-frame CLI input rejected with exit code 2. Usage and timing
boundaries are documented in [the guide](../docs/terminal-headless-benchmark.md).
