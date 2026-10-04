#!/usr/bin/env python3
"""Run isolated bare-terminal comparisons on a Niri/NVIDIA desktop."""

import argparse
import csv
import json
import os
from pathlib import Path
import shutil
import statistics
import subprocess
import sys
import threading
import time

ROOT = Path(__file__).resolve().parent.parent


def write_json(path, value):
    path.write_text(json.dumps(value, indent=2) + "\n")


def capture(*command):
    return subprocess.check_output(command, text=True, timeout=15).strip()


def wait_file(path, timeout=30, process=None):
    deadline = time.monotonic() + timeout
    while not path.exists():
        if process is not None and process.poll() is not None:
            raise RuntimeError(f"Process exited before {path}: {process.returncode}")
        if time.monotonic() > deadline:
            raise TimeoutError(str(path))
        time.sleep(0.02)


def worker(args):
    directory = args.directory
    wait_file(directory / "inspect")
    size = os.get_terminal_size()
    write_json(directory / "grid.json", dict(columns=size.columns, rows=size.lines))
    wait_file(directory / "go")
    sys.stdout.write("\x1b[?25l\x1b[2J\x1b[H")
    sys.stdout.flush()
    started = time.monotonic()
    binary = str(ROOT / "target/release/examples/terminal_benchmark")
    options = ["--columns", str(size.columns)]
    name = args.workload
    command = None
    if name.startswith(("paced-", "flood-")):
        mode, profile = name.split("-")
        command = [binary, "text", "--mode", mode, "--profile", profile, *options]
        command += (["--duration-ms", str(int(args.seconds * 1000)), "--fps", str(args.hz)]
                    if mode == "paced" else ["--lines", str(args.lines)])
    elif name.startswith("image-"):
        command = [binary, "image", "--format", name[6:], "--hold-ms",
                   str(int(args.seconds * 1000)), *options]
    elif name.startswith("animation-"):
        command = [binary, "animation", "--frame-ms", name[10:], "--hold-ms",
                   str(int(args.seconds * 1000)), *options]
    (directory / "started").touch()
    if command:
        with (directory / "producer.txt").open("w") as log:
            subprocess.run(command, stderr=log, check=True)
    elif name == "watch-clear":
        frame = 0
        while time.monotonic() - started < args.seconds:
            sys.stdout.write("\x1b[H\x1b[2JEvery 0.2s: deterministic table\r\n")
            for row in range(min(size.lines - 2, 30)):
                sys.stdout.write(f"| {row:3d} | GPU {frame:06d} | memory {frame % 100:3d}% |\r\n")
            sys.stdout.flush()
            frame += 1
            time.sleep(0.2)
    elif name == "cursor":
        sys.stdout.write("\x1b[?25h")
        frame = 0
        while time.monotonic() - started < args.seconds:
            sys.stdout.write(f"\x1b[{1 + frame % size.lines};{1 + frame % size.columns}H")
            sys.stdout.flush()
            frame += 1
            time.sleep(0.03)
    else:
        time.sleep(args.seconds)
    write_json(directory / "done.json", {"producer_seconds": time.monotonic() - started})
    wait_file(directory / "exit", timeout=120)


def snapshot(pid):
    fields = Path(f"/proc/{pid}/stat").read_text().rsplit(")", 1)[1].split()
    status = dict(line.split(":", 1) for line in Path(f"/proc/{pid}/status").read_text().splitlines())
    return time.monotonic(), int(fields[11]) + int(fields[12]), int(status["VmRSS"].split()[0])


def gpu_samples(directory, pid, stop):
    monitor = subprocess.Popen(["nvidia-smi", "pmon", "-s", "um", "-d", "1"],
                               stdout=subprocess.PIPE, stderr=subprocess.STDOUT, text=True)
    with (directory / "gpu.csv").open("w") as stream:
        writer = csv.DictWriter(stream, fieldnames=["time", "phase", "sm_percent", "vram_mib",
                                                    "board_gpu_percent", "board_power_w", "pstate"])
        writer.writeheader()
        try:
            for raw in monitor.stdout:
                if stop.is_set():
                    break
                phase = "post" if (directory / "done.json").exists() else "workload"
                with (directory / "pmon.txt").open("a") as log:
                    log.write(f"{time.time()} {phase} {raw}")
                match = raw.split()
                if len(match) < 11 or match[1] != str(pid):
                    continue
                board = capture("nvidia-smi", "--query-gpu=utilization.gpu,power.draw,pstate",
                                "--format=csv,noheader,nounits").splitlines()[0].split(", ")
                writer.writerow(dict(time=time.time(), phase=phase,
                                     sm_percent=match[3], vram_mib=match[9],
                                     board_gpu_percent=board[0], board_power_w=board[1], pstate=board[2]))
                stream.flush()
        except (OSError, subprocess.SubprocessError) as error:
            (directory / "gpu-error.txt").write_text(str(error))
        finally:
            monitor.terminate()
            monitor.wait(timeout=10)


def niri(*args):
    return capture("niri", "msg", *args)


def run_case(args, run_dir, terminal, workload, repetition, expected):
    directory = run_dir / f"{repetition:02d}-{workload}-{terminal}"
    attempt = 1
    while directory.exists():
        directory = run_dir / f"{repetition:02d}-{workload}-{terminal}-retry-{attempt}"
        attempt += 1
    directory.mkdir()
    config_home = directory / "config"
    (config_home / "germinal").mkdir(parents=True)
    command = [sys.executable, str(Path(__file__).resolve()), "worker", str(directory), workload,
               "--seconds", str(args.seconds), "--hz", str(args.hz), "--lines", str(args.lines)]
    env = os.environ.copy()
    env.update(XDG_CONFIG_HOME=str(config_home), XDG_STATE_HOME=str(directory / "state"))
    env.pop("GERMINAL_RENDER_PERF_LOG", None)
    env.pop("GERMINAL_TERMINAL_WORKER_PERF_LOG", None)
    germinal_config = f'''[window]
opacity = 1.0
[font]
size = {args.font_size}
ligatures = false
[font.normal]
family = {json.dumps(args.font)}
[cursor]
blinking = false
motion_duration_ms = 0
[rendering]
power_preference = "high"
[scrolling]
history = 10000
[terminal.shell]
program = {json.dumps(command[0])}
args = {json.dumps(command[1:])}
'''
    (config_home / "germinal/config.toml").write_text(germinal_config)
    kitty_config = directory / "kitty.conf"
    kitty_config.write_text(f'''font_family {args.font}
font_size {args.kitty_font_size}
disable_ligatures always
cursor_blink_interval 0
cursor_trail 0
background_opacity 1
window_padding_width 0
confirm_os_window_close 0
linux_display_server wayland
remember_window_size no
scrollback_lines 10000
''')
    launch = ([str(ROOT / "target/release/germinal")] if terminal == "germinal" else
              [shutil.which("kitty"), "--config", str(kitty_config), *command])
    print(f"{directory.name}: launching", flush=True)
    stop = threading.Event()
    sampler = None
    with (directory / "terminal.log").open("w") as log:
        process = subprocess.Popen(launch, env=env, stdout=log, stderr=log)
        try:
            deadline = time.monotonic() + 30
            window = None
            while time.monotonic() < deadline:
                window = next((w for w in json.loads(niri("--json", "windows")) if w["pid"] == process.pid), None)
                if window:
                    break
                if process.poll() is not None:
                    raise RuntimeError(f"Terminal exited: {log.name}")
                time.sleep(0.1)
            if not window:
                raise RuntimeError("No compositor window found")
            niri("action", "fullscreen-window", "--id", str(window["id"]))
            time.sleep(2)
            (directory / "inspect").touch()
            wait_file(directory / "grid.json", process=process)
            grid = json.loads((directory / "grid.json").read_text())
            window = next(w for w in json.loads(niri("--json", "windows")) if w["id"] == window["id"])
            write_json(directory / "window.json", window)
            if not window["is_focused"]:
                raise RuntimeError("Test window lost focus before sampling")
            geometry = {**grid, "window_size": window["layout"]["window_size"]}
            if expected is not None and geometry != expected:
                raise RuntimeError(f"Geometry mismatch: {geometry}; expected {expected}. Adjust font sizes.")
            (directory / "go").touch()
            wait_file(directory / "started", process=process)
            sampler = threading.Thread(target=gpu_samples, args=(directory, process.pid, stop))
            sampler.start()
            previous = snapshot(process.pid)
            ticks = os.sysconf("SC_CLK_TCK")
            samples = []
            done_at = None
            captured = False
            sampling_started = time.monotonic()
            deadline = time.monotonic() + 120
            while time.monotonic() < deadline:
                time.sleep(0.1)
                current = snapshot(process.pid)
                elapsed = current[0] - previous[0]
                phase = "post" if (directory / "done.json").exists() else "workload"
                samples.append(dict(time=current[0], phase=phase, elapsed=elapsed,
                                    cpu_percent=(current[1] - previous[1]) / ticks / elapsed * 100,
                                    rss_mib=current[2] / 1024))
                previous = current
                if repetition == 0 and not captured and time.monotonic() - sampling_started >= 1:
                    if shutil.which("grim"):
                        subprocess.run(["grim", str(directory / "during.png")], check=True)
                    captured = True
                if phase == "post":
                    done_at = done_at or time.monotonic()
                    if time.monotonic() - done_at > 3:
                        break
            if done_at is None:
                raise TimeoutError("Workload did not finish")
            stop.set()
            sampler.join(timeout=15)
            if sampler.is_alive():
                raise RuntimeError("GPU sampler failed to stop")
            with (directory / "cpu.csv").open("w") as stream:
                writer = csv.DictWriter(stream, fieldnames=samples[0].keys())
                writer.writeheader()
                writer.writerows(samples)
            final_window = next(w for w in json.loads(niri("--json", "windows")) if w["id"] == window["id"])
            write_json(directory / "final-window.json", final_window)
            if not final_window["is_focused"] or final_window["layout"]["window_size"] != geometry["window_size"]:
                raise RuntimeError("Test window lost focus or changed size during sampling")
            if repetition == 0 and shutil.which("grim"):
                subprocess.run(["grim", str(directory / "final.png")], check=True)
            (directory / "exit").touch()
            process.wait(timeout=15)
            if process.returncode != 0:
                raise RuntimeError(f"Terminal exited with {process.returncode}")
            measured = [s for s in samples if s["phase"] == "workload"]
            duration = sum(s["elapsed"] for s in measured)
            summary = dict(terminal=terminal, workload=workload, repetition=repetition,
                           cpu_percent=sum(s["cpu_percent"] * s["elapsed"] for s in measured) / duration if duration else None,
                           rss_mib=max(s["rss_mib"] for s in samples),
                           cpu_total_seconds=sum(s["cpu_percent"] * s["elapsed"] for s in samples) / 100,
                           **json.loads((directory / "done.json").read_text()))
            gpu = list(csv.DictReader((directory / "gpu.csv").open()))
            for metric in ["sm_percent", "vram_mib", "board_gpu_percent", "board_power_w"]:
                values = [float(s[metric]) for s in gpu if s["phase"] == "workload" and s[metric] not in ("", "-", "N/A")]
                summary[metric] = statistics.mean(values) if values else None
            write_json(directory / "summary.json", summary)
            print(json.dumps(summary), flush=True)
            return geometry, summary
        except BaseException as error:
            write_json(directory / "invalid.json", {"error": str(error), "type": type(error).__name__})
            raise
        finally:
            stop.set()
            if sampler:
                sampler.join(timeout=15)
            if process.poll() is None:
                process.terminate()
                try:
                    process.wait(timeout=10)
                except subprocess.TimeoutExpired:
                    process.kill()
                    process.wait()


def report(run_dir, summaries):
    metrics = ["cpu_percent", "rss_mib", "cpu_total_seconds", "producer_seconds", "sm_percent", "vram_mib", "board_power_w"]
    rows = ["# Bare terminal comparison", "", "Repetition 0 is an unrecorded warm-up and is excluded below.",
            "Board power includes all desktop processes. Missing GPU counters remain unavailable, not zero.",
            "Flood duration measures producer delivery, not completed presentation. CPU total includes a three-second drain period.",
            "", "| Workload | Terminal | Runs | CPU % | RSS MiB | CPU s | Producer s | GPU SM % | VRAM MiB | Board W |",
            "|---|---|---:|---:|---:|---:|---:|---:|---:|---:|---:|"]
    for workload in dict.fromkeys(s["workload"] for s in summaries):
        for terminal in ("germinal", "kitty"):
            selected = [s for s in summaries if s["repetition"] > 0 and s["terminal"] == terminal and s["workload"] == workload]
            if not selected:
                continue
            values = []
            for metric in metrics:
                numbers = [s[metric] for s in selected if s[metric] is not None]
                values.append(f"{statistics.median(numbers):.2f}" if numbers else "N/A")
            rows.append("| " + " | ".join([workload, terminal, str(len(selected)), *values]) + " |")
    (run_dir / "summary.md").write_text("\n".join(rows) + "\n")


def run(args):
    run_dir = args.output.resolve()
    if args.resume:
        old = json.loads((run_dir / "environment.json").read_text())
        for key, value in vars(args).items():
            if key not in ("resume", "output") and old["arguments"][key] != value:
                raise RuntimeError(f"Resume parameter mismatch: {key}")
        if old["revision"] != capture("git", "rev-parse", "HEAD"):
            raise RuntimeError("Revision changed since the original run")
        if old["outputs"] != json.loads(niri("--json", "outputs")):
            raise RuntimeError("Display configuration changed since the original run")
        summaries = json.loads((run_dir / "summary.json").read_text())
        first = next(path.parent for path in sorted(run_dir.glob("*/summary.json")))
        expected = {**json.loads((first / "grid.json").read_text()),
                    "window_size": json.loads((first / "window.json").read_text())["layout"]["window_size"]}
        write_json(run_dir / f"resume-{time.time_ns()}.json", {"time": time.time(), "revision": old["revision"]})
    else:
        run_dir.mkdir(parents=True, exist_ok=False)
        summaries = []
        expected = None
    environment = dict(revision=capture("git", "rev-parse", "HEAD"),
                       dirty=capture("git", "status", "--short"),
                       outputs=json.loads(niri("--json", "outputs")),
                       gpu=capture("nvidia-smi"), kitty=capture("kitty", "--version"),
                       font=capture("fc-match", "-f", "%{file}\n", args.font), arguments=vars(args).copy())
    environment["arguments"]["output"] = str(args.output)
    if not args.resume:
        write_json(run_dir / "environment.json", environment)
    for repetition in range(args.repeats + 1):
        for workload in args.workloads:
            order = ["germinal", "kitty"] if repetition % 2 == 0 else ["kitty", "germinal"]
            for terminal in order:
                if any(s["repetition"] == repetition and s["workload"] == workload and s["terminal"] == terminal for s in summaries):
                    continue
                expected, summary = run_case(args, run_dir, terminal, workload, repetition, expected)
                summaries.append(summary)
                write_json(run_dir / "summary.json", summaries)
                report(run_dir, summaries)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    sub = parser.add_subparsers(dest="action", required=True)
    run_parser = sub.add_parser("run")
    run_parser.add_argument("--output", type=Path, required=True)
    run_parser.add_argument("--resume", action="store_true")
    run_parser.add_argument("--repeats", type=int, default=5)
    run_parser.add_argument("--font", default="JetBrainsMono Nerd Font")
    run_parser.add_argument("--font-size", type=float, default=20)
    run_parser.add_argument("--kitty-font-size", type=float, default=15)
    workloads = ["idle", "watch-clear", "paced-ascii", "paced-unicode", "flood-ascii", "flood-unicode",
                 "cursor", "image-rgba", "image-png", "animation-8", "animation-16"]
    run_parser.add_argument("--workloads", nargs="+", choices=workloads, default=workloads)
    worker_parser = sub.add_parser("worker")
    worker_parser.add_argument("directory", type=Path)
    worker_parser.add_argument("workload", choices=workloads)
    for child in (run_parser, worker_parser):
        child.add_argument("--seconds", type=float, default=10)
        child.add_argument("--hz", type=int, default=165)
        child.add_argument("--lines", type=int, default=250000)
    args = parser.parse_args()
    if args.seconds <= 0 or args.hz <= 0 or args.lines <= 0:
        parser.error("seconds, hz and lines must be positive")
    if args.action == "run" and args.repeats < 0:
        parser.error("repeats must be nonnegative")
    (run if args.action == "run" else worker)(args)


if __name__ == "__main__":
    main()
