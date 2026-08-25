# Germinal vs. Kitty Benchmark Specification

This document is an execution specification for an AI agent. It is not a benchmark report,
compatibility report, or product document. Execute the procedure, retain the raw artifacts, and
derive conclusions only from the resulting measurements.

## Objective

The measured products are Germinal and Kitty. Run both directly and through the same Zellij layer:

| Case | Host terminal | Intermediate layer |
| --- | --- | --- |
| `germinal-bare` | Germinal | None |
| `kitty-bare` | Kitty | None |
| `germinal-zellij` | Germinal | Zellij |
| `kitty-zellij` | Kitty | Zellij |

Zellij is a shared workload layer, not a third product under comparison. Run text, static-image,
image-animation, and interactive workloads in all four cases.

## Output layout

Create a unique `run-id` before starting and store every artifact under one directory:

```text
results/<run-id>/
  environment.json
  notes.md
  raw/
    <case>-<workload>-run-<n>.csv
    <case>-<workload>-run-<n>.txt
  summary.md
```

Never overwrite an earlier run. The `txt` files contain workload output and `BENCH_RESULT`; the
`csv` files contain CPU and memory samples. Do not retain only the summary.

## Controlled variables

Keep these conditions identical across all four cases:

- display output, refresh rate, scale, power profile, and GPU selection;
- physical window size and maximized/fullscreen state;
- row and column count reported by `stty size`;
- font files, font size, fallback fonts, and ligature settings;
- opaque background with background images disabled;
- one window, one tab, and one pane;
- the same Zellij version, configuration, and empty layout for both Zellij cases;
- one unrecorded warm-up before measurement;
- release builds with Germinal internal performance logging disabled;
- no compilation, download, screen recording, or unrelated high-load process during sampling.

If any condition cannot be matched, stop formal sampling and record the mismatch in `notes.md`.
Do not continue silently.

## Record the environment

Run:

```nu
./scripts/terminal-benchmark.nu environment \
    --output results/<run-id>/environment.json \
    --display "<display-name-and-resolution>" \
    --refresh-hz <hz> \
    --scale <scale>
```

Add these fields to `notes.md`:

```text
Physical window size in pixels:
stty size:
Font family, exact font files, and font size:
Germinal configuration path and benchmark overrides:
Kitty configuration path and benchmark overrides:
Zellij version:
Power profile:
Known visible problems before sampling:
```

## Build the workload generator

```nu
cargo build --locked --release -p germinal --example terminal_benchmark
```

Run the built binary directly during measurement:

```text
target/release/examples/terminal_benchmark
```

Do not use `cargo run` while sampling. Record the Git revision and dirty state. List uncommitted
changes, but do not abort solely because the worktree is dirty.

## Prepare each case

Run bare cases directly in Germinal or Kitty. For each Zellij case, start a fresh session from the
bare host terminal:

```nu
zellij --session terminal-bench-<case>-<run> \
    --config benchmarks/zellij.kdl \
    --layout benchmarks/zellij-layout.kdl
```

Do not attach to an existing session and do not nest Zellij. Before each measured run, identify the
host terminal PID. For a Zellij case, also identify the Zellij server PID:

```nu
./scripts/terminal-benchmark.nu processes
```

Sample only the host PID for a bare case. Sum the host and Zellij server processes for a Zellij
case:

```nu
./scripts/terminal-benchmark.nu sample <case> <terminal-pid> \
    --duration 20sec \
    --output results/<run-id>/raw/<case>-<workload>-run-<n>.csv

./scripts/terminal-benchmark.nu sample <case> <terminal-pid> <zellij-pid> \
    --duration 20sec \
    --output results/<run-id>/raw/<case>-<workload>-run-<n>.csv
```

Do not include the workload-generator process in terminal CPU or memory totals.

## Execution rules

Apply these rules to every workload:

1. Run one unrecorded warm-up in every case.
2. Run five measured repetitions in every case.
3. Rotate case order to reduce thermal and ordering bias.
4. Save the CSV, complete workload output, `BENCH_RESULT`, and visible anomalies for every run.
5. Report the median of five repetitions and retain the worst CPU, RSS, and frame-pacing run.
6. Mark a run with incomplete or incorrect visible output as `invalid`, but retain its artifacts.
7. Never treat low resource use from an invalid run as a performance advantage.
8. Treat visual validity only as a benchmark validity gate; do not turn it into a Zellij
   compatibility conclusion.

Use this rotation:

```text
run 1: germinal-bare -> kitty-bare -> germinal-zellij -> kitty-zellij
run 2: kitty-bare -> germinal-zellij -> kitty-zellij -> germinal-bare
run 3: germinal-zellij -> kitty-zellij -> germinal-bare -> kitty-bare
run 4: kitty-zellij -> germinal-bare -> kitty-bare -> germinal-zellij
run 5: repeat the run 1 order
```

## Workload A: ASCII text throughput

```nu
target/release/examples/terminal_benchmark text \
    --mode flood --profile ascii --lines 250000 --columns 120
```

Record producer MiB/s, average and peak CPU, peak RSS, and the time for which the terminal remains
busy after `BENCH_RESULT` appears. Producer time measures data delivery, not presentation latency
or FPS.

## Workload B: Unicode text throughput

```nu
target/release/examples/terminal_benchmark text \
    --mode flood --profile unicode --lines 250000 --columns 120
```

In addition to throughput metrics, record missing glyphs, incorrect fallback, glyph positioning
errors, and cold glyph-atlas behavior.

## Workload C: paced text scrolling

Set `--fps` to the active display refresh rate:

```nu
target/release/examples/terminal_benchmark text \
    --mode paced --profile ascii --duration-ms 15000 \
    --fps <refresh-hz> --columns 120
```

Record producer deadline misses, CPU, RSS, pauses, catch-up bursts, and uneven scrolling. Use an
external high-speed camera when long-frame quantification is required. Desktop recording changes
the workload and must not be used for the primary result.

## Workload D: static RGBA image

```nu
target/release/examples/terminal_benchmark image \
    --format rgba --width 960 --height 540 \
    --columns 120 --rows 30 --hold-ms 10000
```

Record transfer time, upload-stage CPU, idle display CPU, peak RSS, and time until the complete
image first becomes visible.

## Workload E: static PNG image

```nu
target/release/examples/terminal_benchmark image \
    --format png --width 960 --height 540 \
    --columns 120 --rows 30 --hold-ms 10000
```

Use the same image content and dimensions as the RGBA workload. The pair separates PNG decode cost
from raw-pixel transfer cost.

## Workload F: terminal-driven image animation

Requested 125 Hz interval:

```nu
target/release/examples/terminal_benchmark animation \
    --width 640 --height 360 --frames 12 --frame-ms 8 \
    --columns 120 --rows 30 --hold-ms 15000
```

Approximately 60 Hz control:

```nu
target/release/examples/terminal_benchmark animation \
    --width 640 --height 360 --frames 12 --frame-ms 16 \
    --columns 120 --rows 30 --hold-ms 15000
```

Measure upload and looping CPU/RSS separately. Observe frame order, motion uniformity, and loop
boundary pauses. An 8 ms requested interval is not proof of 125 displayed FPS. The Zellij cases
measure the complete host-plus-multiplexer cost and do not replace bare-terminal rendering data.

## Workload G: interaction and cursor motion

Use fixed event intervals and this input sequence:

1. Type 80 characters continuously.
2. Press Backspace 40 times.
3. Move left 40 times and right 40 times.
4. Move across lines with Up and Down 20 times each.
5. Press Enter 20 times.
6. Repeat steps 1 through 5 in a short window.

First disable cursor animation in both Germinal and Kitty to measure base input response. Then
enable each terminal's default cursor animation to measure visual continuity and additional CPU.
Record first response, animation interruption, catch-up, movement amplitude, and peak CPU. When
input is automated, save the exact key sequence, modifiers, and event intervals.

## Summary tables

Produce this table for every workload. Do not omit invalid runs:

| Case | Valid runs | Producer median | CPU avg median | CPU peak worst | RSS peak median | RSS peak worst | Visual notes |
| --- | ---: | ---: | ---: | ---: | ---: | ---: | --- |
| `germinal-bare` |  |  |  |  |  |  |  |
| `kitty-bare` |  |  |  |  |  |  |  |
| `germinal-zellij` |  |  |  |  |  |  |  |
| `kitty-zellij` |  |  |  |  |  |  |  |

Add this table for paced text, image animation, and cursor motion:

| Case | Requested Hz | Deadline misses | Long/repeated frames | Stutter location |
| --- | ---: | ---: | ---: | --- |
| `germinal-bare` |  |  |  |  |
| `kitty-bare` |  |  |  |  |
| `germinal-zellij` |  |  |  |  |
| `kitty-zellij` |  |  |  |  |

## Analysis rules

Calculate only these comparison groups:

```text
bare gap = germinal-bare compared with kitty-bare
zellij gap = germinal-zellij compared with kitty-zellij
zellij delta = zellij case compared with bare case for the same host terminal
```

Keep these distinctions explicit:

- producer throughput versus presentation completion time;
- average CPU versus peak CPU;
- image upload cost versus idle display cost;
- requested animation frequency versus measured frame intervals;
- smoothness defects versus resource-efficiency defects;
- bare-terminal differences versus host-plus-Zellij differences.

The final `summary.md` may contain measured values, deltas, plausible causes, and hypotheses that
still require verification. Do not claim that Germinal has matched Kitty, is faster than Kitty, or
has a compatibility defect without supporting measurements.

## Germinal diagnostic rerun

Only after the primary benchmark is complete, enable internal logging for a diagnostic rerun when
Germinal shows a material gap:

```nu
with-env {
    GERMINAL_TERMINAL_WORKER_PERF_LOG: "1"
    GERMINAL_RENDER_PERF_LOG: "1"
} {
    target/product/germinal
}
```

Use internal logs to inspect PTY apply/publish work, wakeup coalescing, prepare/upload/render time,
surface presentation, and glyph-atlas hits. Kitty has no directly equivalent values, so do not put
these numbers in the primary comparison table or enable these logs during primary sampling.

## References

- [Kitty performance](https://sw.kovidgoyal.net/kitty/performance/)
- [Kitty Graphics Protocol and animation](https://sw.kovidgoyal.net/kitty/graphics-protocol/)
- [Zellij options](https://zellij.dev/documentation/options.html)
