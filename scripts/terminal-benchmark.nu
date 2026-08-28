#!/usr/bin/env nu

def fail [message: string, code: int = 1] {
    print --stderr $message
    exit $code
}

def capture-version [program: string, ...args: string] {
    if (which $program | is-empty) {
        return "not installed"
    }
    let result = (run-external $program ...$args | complete)
    if $result.exit_code != 0 {
        return $"error (exit ($result.exit_code))"
    }
    $result.stdout | str trim
}

def optional-command-output [program: string, ...args: string] {
    if (which $program | is-empty) {
        return null
    }
    let result = (run-external $program ...$args | complete)
    if $result.exit_code == 0 {
        $result.stdout | str trim
    } else {
        null
    }
}

def git-revision [] {
    let result = (run-external git "rev-parse" "HEAD" | complete)
    if $result.exit_code == 0 {
        $result.stdout | str trim
    } else {
        "not a git checkout"
    }
}

def git-dirty [] {
    let result = (run-external git "status" "--short" | complete)
    $result.exit_code == 0 and (not ($result.stdout | str trim | is-empty))
}

def environment-value [name: string] {
    $env | get --ignore-errors $name | default ""
}

def ensure-output-parent [output: path] {
    let parent = ($output | path dirname)
    if not ($parent | path exists) {
        mkdir $parent
    }
}

def wait-main-pid [unit: string, timeout: duration = 10sec] {
    let deadline = (date now) + $timeout
    while (date now) < $deadline {
        let result = (^systemctl --user show $unit --property MainPID --value | complete)
        if $result.exit_code == 0 {
            let pid = try {
                $result.stdout | str trim | into int
            } catch {
                0
            }
            if $pid > 0 {
                return $pid
            }
        }
        sleep 100ms
    }
    fail $"Timed out waiting for ($unit) to expose its MainPID."
}

def wait-path [path: path, timeout: duration = 30sec] {
    let deadline = (date now) + $timeout
    while (date now) < $deadline {
        if ($path | path exists) {
            return
        }
        sleep 10ms
    }
    fail $"Timed out waiting for ($path)."
}

def process-snapshot [pid: int] {
    let stat_path = $"/proc/($pid)/stat"
    if not ($stat_path | path exists) {
        return null
    }

    let stat = (open --raw $stat_path)
    let fields = (
        $stat
        | str replace --regex '^\d+ \(.*\) ' ''
        | str trim
        | split row --regex '\s+'
    )
    if ($fields | length) < 13 {
        return null
    }

    let status_path = $"/proc/($pid)/status"
    let status = if ($status_path | path exists) {
        open --raw $status_path | lines
    } else {
        []
    }
    let rss = (
        $status
        | where {|line| $line starts-with "VmRSS:" }
        | first
        | parse --regex 'VmRSS:\s+(?<value>\d+)'
        | get 0.value
        | into int
    )
    let virtual = (
        $status
        | where {|line| $line starts-with "VmSize:" }
        | first
        | parse --regex 'VmSize:\s+(?<value>\d+)'
        | get 0.value
        | into int
    )

    {
        pid: $pid
        timestamp_ns: (date now | into int)
        cpu_ticks: (($fields | get 11 | into int) + ($fields | get 12 | into int))
        rss_kib: $rss
        virtual_kib: $virtual
    }
}

def sample-process [
    pid: int
    signal_dir: path
    --interval: duration = 100ms
    --timeout: duration = 2min
] {
    let ticks_per_second = (^getconf CLK_TCK | str trim | into int)
    let started_path = ($signal_dir | path join "started")
    let done_path = ($signal_dir | path join "done")
    let deadline = (date now) + $timeout
    mut previous = (process-snapshot $pid)
    mut samples = []
    mut timed_out = false

    if $previous == null {
        fail $"Process ($pid) disappeared before sampling started."
    }

    loop {
        sleep $interval
        if (date now) >= $deadline {
            $timed_out = true
            break
        }
        let current = (process-snapshot $pid)
        if $current == null {
            break
        }

        let elapsed_ns = $current.timestamp_ns - $previous.timestamp_ns
        let cpu_ticks = $current.cpu_ticks - $previous.cpu_ticks
        let cpu_percent = if $elapsed_ns > 0 {
            ($cpu_ticks * 100.0 * 1_000_000_000.0) / ($ticks_per_second * $elapsed_ns)
        } else {
            0.0
        }
        let phase = if ($done_path | path exists) {
            "post"
        } else if ($started_path | path exists) {
            "workload"
        } else {
            "startup"
        }
        $samples = ($samples | append {
            timestamp_ns: $current.timestamp_ns
            phase: $phase
            elapsed_ns: $elapsed_ns
            cpu_ticks_delta: $cpu_ticks
            cpu_percent: $cpu_percent
            rss_kib: $current.rss_kib
        })
        $previous = $current
    }

    { samples: $samples, timed_out: $timed_out, ticks_per_second: $ticks_per_second }
}

def summarize-case [
    case_name: string
    sampled: record
    result_path: path
] {
    let workload = ($sampled.samples | where phase == "workload")
    if ($workload | is-empty) {
        fail $"No workload samples were captured for ($case_name)."
    }

    let elapsed_ns = ($workload.elapsed_ns | math sum)
    let cpu_ticks = ($workload.cpu_ticks_delta | math sum)
    let cpu_avg = if $elapsed_ns > 0 {
        ($cpu_ticks * 100.0 * 1_000_000_000.0) / ($sampled.ticks_per_second * $elapsed_ns)
    } else {
        0.0
    }
    let result = if ($result_path | path exists) {
        open --raw $result_path | str trim
    } else {
        ""
    }

    {
        case: $case_name
        samples: ($workload | length)
        cpu_avg_percent: ($cpu_avg | math round --precision 2)
        cpu_peak_percent: ($workload.cpu_percent | math max | math round --precision 2)
        rss_peak_mib: ((($workload.rss_kib | math max) / 1024.0) | math round --precision 2)
        timed_out: $sampled.timed_out
        result: $result
    }
}

def write-suite-config [
    output: path
    script: path
    benchmark: path
    signal_dir: path
    workload: string
    text_profile: string
    shader: string
    refresh_hz: int
    idle_ms: int
    paced_duration_ms: int
    flood_lines: int
    columns: int
    warmup_ms: int
    post_hold_ms: int
    font_family: string
    font_size: float
] {
    let shell_args = [
        "worker"
        $workload
        "--benchmark"
        ($benchmark | into string)
        "--signal-dir"
        ($signal_dir | into string)
        "--text-profile"
        $text_profile
        "--refresh-hz"
        ($refresh_hz | into string)
        "--idle-ms"
        ($idle_ms | into string)
        "--paced-duration-ms"
        ($paced_duration_ms | into string)
        "--flood-lines"
        ($flood_lines | into string)
        "--columns"
        ($columns | into string)
        "--warmup-ms"
        ($warmup_ms | into string)
        "--post-hold-ms"
        ($post_hold_ms | into string)
    ]
    let font = if ($font_family | str trim | is-empty) {
        { size: $font_size }
    } else {
        { size: $font_size, normal: { family: $font_family } }
    }
    let base = {
        window: {
            title: $"Germinal benchmark: ($workload)"
            width_px: 960
            height_px: 540
            opacity: 1.0
        }
        font: $font
        terminal: {
            shell: {
                program: ($script | into string)
                args: $shell_args
            }
        }
        logging: {
            console_level: "info"
            file_level: "info"
        }
    }
    let config = if ($shader | str trim | is-empty) {
        $base
    } else {
        $base | merge { background: { shader: $shader } }
    }
    ensure-output-parent $output
    $config | to toml | save --force $output
}

def run-suite-case [
    case: record
    index: int
    run_dir: path
    repo_root: path
    script: path
    germinal: path
    benchmark: path
    text_profile: string
    refresh_hz: int
    idle_ms: int
    paced_duration_ms: int
    flood_lines: int
    columns: int
    warmup_ms: int
    post_hold_ms: int
    interval: duration
    font_family: string
    font_size: float
    perf_seconds: int
    perf_frequency: int
] {
    let case_dir = ($run_dir | path join $case.name)
    let config_home = ($case_dir | path join "config")
    let state_home = ($case_dir | path join "state")
    let signal_dir = ($case_dir | path join "signals")
    let config_path = ($config_home | path join "germinal" "config.toml")
    let raw_path = ($case_dir | path join "samples.csv")
    let perf_path = ($case_dir | path join "perf.data")
    let result_path = ($signal_dir | path join "result.txt")
    let started_path = ($signal_dir | path join "started")
    mkdir $signal_dir
    write-suite-config $config_path $script $benchmark $signal_dir $case.workload $text_profile $case.shader $refresh_hz $idle_ms $paced_duration_ms $flood_lines $columns $warmup_ms $post_hold_ms $font_family $font_size

    let unit = $"germinal-bench-($nu.pid)-($index).service"
    mut launch_args = [
        "--user"
        "--quiet"
        "--no-block"
        "--collect"
        "--service-type=exec"
        $"--unit=($unit)"
        $"--working-directory=($repo_root)"
        "--property=KillMode=mixed"
        $"--setenv=XDG_CONFIG_HOME=($config_home)"
        $"--setenv=XDG_STATE_HOME=($state_home)"
    ]
    for name in [
        "WAYLAND_DISPLAY"
        "DISPLAY"
        "XDG_RUNTIME_DIR"
        "XDG_SESSION_TYPE"
        "XDG_CURRENT_DESKTOP"
        "DBUS_SESSION_BUS_ADDRESS"
        "XAUTHORITY"
    ] {
        let value = (environment-value $name)
        if not ($value | is-empty) {
            $launch_args = ($launch_args | append $"--setenv=($name)=($value)")
        }
    }

    print $"Starting ($case.name)..."
    let launch = (^systemd-run ...$launch_args $germinal | complete)
    if $launch.exit_code != 0 {
        fail $"Failed to start ($case.name): ($launch.stderr | str trim)"
    }
    let pid = (wait-main-pid $unit)
    let perf_unit = $"germinal-bench-perf-($nu.pid)-($index).service"
    let should_profile = $perf_seconds > 0 and $case.workload == "flood-ascii"
    if $should_profile {
        wait-path $started_path
        let perf_launch = (
            ^systemd-run
                --user
                --quiet
                --no-block
                --collect
                --service-type=exec
                $"--unit=($perf_unit)"
                perf record
                --output $perf_path
                --freq $perf_frequency
                --call-graph fp
                --pid $pid
                -- sleep $perf_seconds
            | complete
        )
        if $perf_launch.exit_code != 0 {
            let _ = (^systemctl --user stop $unit | complete)
            fail $"Failed to start perf for ($case.name): ($perf_launch.stderr | str trim)"
        }
    }
    let sampled = (sample-process $pid $signal_dir --interval $interval)
    $sampled.samples | to csv | save --force $raw_path
    let _ = (^systemctl --user stop $unit | complete)
    if $should_profile {
        let _ = (^systemctl --user stop $perf_unit | complete)
        if not ($perf_path | path exists) {
            fail $"perf did not produce ($perf_path)."
        }
        print $"perf data: ($perf_path)"
    }

    let summary = (summarize-case $case.name $sampled $result_path)
    print $summary
    $summary
}

# Record the software and hardware context needed to compare benchmark runs.
def "main environment" [
    --output: path = "terminal-benchmark-environment.json"
    --display: string = "FILL_ME"
    --refresh-hz: float = 0.0
    --scale: float = 0.0
] {
    let cpu = (sys cpu | first)
    let host = (sys host)
    let memory = (sys mem)
    let record = {
        recorded_at: (date now)
        repository_revision: (git-revision)
        repository_dirty: (git-dirty)
        host: $host
        cpu: {
            brand: $cpu.brand
            logical_cpu_count: (sys cpu | length)
        }
        memory_bytes: {
            total: $memory.total
            available_at_start: $memory.available
        }
        gpu_pci: (optional-command-output "lspci" "-nnk" "-d" "::03xx")
        vulkan_summary: (optional-command-output "vulkaninfo" "--summary")
        session: {
            xdg_session_type: (environment-value "XDG_SESSION_TYPE")
            wayland_display: (environment-value "WAYLAND_DISPLAY")
            display: (environment-value "DISPLAY")
            desktop: (environment-value "XDG_CURRENT_DESKTOP")
        }
        measured_display: {
            name: $display
            refresh_hz: $refresh_hz
            scale: $scale
        }
        versions: {
            germinal_revision: (git-revision)
            kitty: (capture-version "kitty" "--version")
            zellij: (capture-version "zellij" "--version")
            nushell: (capture-version "nu" "--version")
            rustc: (capture-version "rustc" "--version")
        }
    }
    ensure-output-parent $output
    $record | to json | save --force $output
    print $"Wrote ($output)"
    if $display == "FILL_ME" or $refresh_hz == 0.0 or $scale == 0.0 {
        print --stderr "Warning: fill --display, --refresh-hz, and --scale before using this run."
    }
}

# Sample CPU and resident memory for one or more explicit process IDs.
def "main sample" [
    label: string
    ...pids: int
    --duration: duration = 15sec
    --interval: duration = 250ms
    --output: path
] {
    if ($pids | is-empty) {
        fail "Pass the terminal PID and, for a Zellij run, the Zellij server PID."
    }
    let output_path = if $output == null {
        $"terminal-benchmark-($label).csv"
    } else {
        $output
    }
    let ticks_per_second = (^getconf CLK_TCK | str trim | into int)
    let deadline = (date now) + $duration
    mut previous = ($pids | each {|pid| process-snapshot $pid })
    mut previous_ns = (date now | into int)
    mut samples = []
    if ($previous | length) != ($pids | length) {
        fail $"Could not read every requested PID: ($pids | str join ', ')."
    }
    while (date now) < $deadline {
        sleep $interval
        let current_ns = (date now | into int)
        let current = ($pids | each {|pid| process-snapshot $pid })
        if ($current | length) != ($pids | length) {
            break
        }
        let elapsed_ns = $current_ns - $previous_ns
        mut cpu_ticks = 0
        for process in $current {
            let before = ($previous | where {|item| $item.pid == $process.pid } | first)
            $cpu_ticks += $process.cpu_ticks - $before.cpu_ticks
        }
        let cpu_percent = if $elapsed_ns > 0 {
            ($cpu_ticks * 100.0 * 1_000_000_000.0) / ($ticks_per_second * $elapsed_ns)
        } else {
            0.0
        }
        $samples = ($samples | append {
            timestamp: (date now)
            label: $label
            requested_pids: ($pids | str join "+")
            found_processes: ($current | length)
            elapsed_ns: $elapsed_ns
            cpu_ticks_delta: $cpu_ticks
            cpu_percent_sum: $cpu_percent
            rss_bytes_sum: (($current.rss_kib | math sum) * 1024)
            virtual_bytes_sum: (($current.virtual_kib | math sum) * 1024)
        })
        $previous = $current
        $previous_ns = $current_ns
    }
    if ($samples | is-empty) {
        fail $"No samples collected for PIDs ($pids | str join ', ')."
    }
    ensure-output-parent $output_path
    $samples | to csv | save --force $output_path
    let cpu = (
        ($samples.cpu_ticks_delta | math sum) * 100.0 * 1_000_000_000.0
    ) / ($ticks_per_second * ($samples.elapsed_ns | math sum))
    let rss_peak = ($samples.rss_bytes_sum | math max)
    print $"Wrote ($output_path): samples=($samples | length) cpu_avg=($cpu | math round --precision 2)% rss_peak_bytes=($rss_peak)"
}

# List candidate host-terminal and Zellij process IDs before starting a sample.
def "main processes" [] {
    ps
    | where {|process| $process.name =~ '(?i)(germinal|kitty|zellij)' }
    | select pid ppid name status cpu mem virtual
}

# Internal PTY entry point used by `germinal`. It runs without desktop input automation.
def "main worker" [
    workload: string
    --benchmark: path
    --signal-dir: path
    --text-profile: string = "ascii"
    --refresh-hz: int = 165
    --idle-ms: int = 10000
    --paced-duration-ms: int = 15000
    --flood-lines: int = 2500000
    --columns: int = 120
    --warmup-ms: int = 1500
    --post-hold-ms: int = 3000
] {
    mkdir $signal_dir
    let started_path = ($signal_dir | path join "started")
    let done_path = ($signal_dir | path join "done")
    let result_path = ($signal_dir | path join "result.txt")

    sleep ($warmup_ms * 1ms)
    touch $started_path
    mut workload_exit_code = 0
    match $workload {
        "idle" => {
            sleep ($idle_ms * 1ms)
        }
        "paced-ascii" => {
            (
                ^$benchmark text
                    --mode paced
                    --profile $text_profile
                    --duration-ms $paced_duration_ms
                    --fps $refresh_hz
                    --columns $columns
                    err> $result_path
            )
            $workload_exit_code = $env.LAST_EXIT_CODE
        }
        "flood-ascii" => {
            (
                ^$benchmark text
                    --mode flood
                    --profile $text_profile
                    --lines $flood_lines
                    --columns $columns
                    err> $result_path
            )
            $workload_exit_code = $env.LAST_EXIT_CODE
        }
        _ => {
            print --stderr $"Unknown suite workload: ($workload)"
            touch $done_path
            exit 2
        }
    }
    touch $done_path
    sleep ($post_hold_ms * 1ms)
    exit $workload_exit_code
}

# Build, launch, sample, summarize, and clean up the complete local Germinal performance suite.
def "main germinal" [
    --output-root: path = "/tmp/germinal-performance"
    --refresh-hz: int = 165
    --text-profile: string = "ascii"
    --idle-ms: int = 10000
    --paced-duration-ms: int = 15000
    --flood-lines: int = 2500000
    --columns: int = 120
    --warmup-ms: int = 1500
    --post-hold-ms: int = 3000
    --interval: duration = 100ms
    --font-family: string = ""
    --font-size: float = 20.0
    --perf-seconds: int = 0
    --perf-frequency: int = 749
    --skip-shader
    --skip-build
] {
    if not ($text_profile in ["ascii" "unicode"]) {
        fail $"--text-profile must be ascii or unicode, got: ($text_profile)"
    }
    if $perf_seconds < 0 {
        fail $"--perf-seconds must not be negative, got: ($perf_seconds)"
    }
    if $perf_frequency < 1 {
        fail $"--perf-frequency must be positive, got: ($perf_frequency)"
    }

    let repo_root = ($env.FILE_PWD | path dirname | path expand)
    let script = ($env.FILE_PWD | path join "terminal-benchmark.nu" | path expand)
    let germinal = ($repo_root | path join "target" "release" "germinal")
    let benchmark = (
        $repo_root
        | path join "target" "release" "examples" "terminal_benchmark"
    )

    if not $skip_build {
        print "Building optimized Germinal and workload generator..."
        let build = (
            ^cargo build --locked --release -p germinal
                --bin germinal
                --example terminal_benchmark
            | complete
        )
        if $build.exit_code != 0 {
            print --stderr $build.stdout
            fail $build.stderr $build.exit_code
        }
    }
    for program in [$germinal $benchmark] {
        if not ($program | path exists) {
            fail $"Required release executable does not exist: ($program)"
        }
    }
    for program in ["systemd-run" "systemctl" "getconf"] {
        if (which $program | is-empty) {
            fail $"Required benchmark command is not installed: ($program)"
        }
    }
    if $perf_seconds > 0 and (which perf | is-empty) {
        fail "--perf-seconds requires perf to be installed."
    }

    let stamp = (date now | format date "%Y%m%d-%H%M%S")
    let run_dir = (
        $output_root
        | path expand
        | path join $"($stamp)-($nu.pid)"
    )
    mkdir $run_dir
    let base_cases = [
        { name: "static-idle", workload: "idle", shader: "" }
        { name: $"static-paced-($text_profile)", workload: "paced-ascii", shader: "" }
        { name: $"static-flood-($text_profile)", workload: "flood-ascii", shader: "" }
    ]
    let cases = if $skip_shader {
        $base_cases
    } else {
        $base_cases | append [
            { name: "starfield-idle", workload: "idle", shader: "starfield" }
            { name: $"starfield-paced-($text_profile)", workload: "paced-ascii", shader: "starfield" }
        ]
    }

    mut summaries = []
    for case in ($cases | enumerate) {
        let summary = (
            run-suite-case
                $case.item
                $case.index
                $run_dir
                $repo_root
                $script
                $germinal
                $benchmark
                $text_profile
                $refresh_hz
                $idle_ms
                $paced_duration_ms
                $flood_lines
                $columns
                $warmup_ms
                $post_hold_ms
                $interval
                $font_family
                $font_size
                $perf_seconds
                $perf_frequency
        )
        $summaries = ($summaries | append $summary)
    }

    let summary_path = ($run_dir | path join "summary.csv")
    $summaries | to csv | save --force $summary_path
    print ""
    print ($summaries | select case samples cpu_avg_percent cpu_peak_percent rss_peak_mib timed_out)
    print $"Raw samples and summary: ($run_dir)"
}

def main [] {
    print "Germinal terminal comparison helper"
    print "  ./scripts/terminal-benchmark.nu environment --display <name> --refresh-hz <hz> --scale <factor>"
    print "  ./scripts/terminal-benchmark.nu processes"
    print "  ./scripts/terminal-benchmark.nu sample <label> <terminal-pid> [zellij-pid]"
    print "  ./scripts/terminal-benchmark.nu germinal"
    print "See docs/performance-comparison.md for the complete procedure."
}
