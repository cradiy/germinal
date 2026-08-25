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
    let deadline = (date now) + $duration
    mut samples = []
    while (date now) < $deadline {
        let processes = (ps | where {|process| $process.pid in $pids })
        if ($processes | is-empty) {
            break
        }
        $samples = ($samples | append {
            timestamp: (date now)
            label: $label
            requested_pids: ($pids | str join "+")
            found_processes: ($processes | length)
            cpu_percent_sum: ($processes.cpu | math sum)
            rss_bytes_sum: ($processes.mem | each {|value| $value | into int } | math sum)
            virtual_bytes_sum: ($processes.virtual | each {|value| $value | into int } | math sum)
        })
        sleep $interval
    }
    if ($samples | is-empty) {
        fail $"No samples collected for PIDs ($pids | str join ', ')."
    }
    ensure-output-parent $output_path
    $samples | to csv | save --force $output_path
    let cpu = ($samples.cpu_percent_sum | math avg)
    let rss_peak = ($samples.rss_bytes_sum | math max)
    print $"Wrote ($output_path): samples=($samples | length) cpu_avg=($cpu | math round --precision 2)% rss_peak_bytes=($rss_peak)"
}

# List candidate host-terminal and Zellij process IDs before starting a sample.
def "main processes" [] {
    ps
    | where {|process| $process.name =~ '(?i)(germinal|kitty|zellij)' }
    | select pid ppid name status cpu mem virtual
}

def main [] {
    print "Germinal terminal comparison helper"
    print "  ./scripts/terminal-benchmark.nu environment --display <name> --refresh-hz <hz> --scale <factor>"
    print "  ./scripts/terminal-benchmark.nu processes"
    print "  ./scripts/terminal-benchmark.nu sample <label> <terminal-pid> [zellij-pid]"
    print "See docs/performance-comparison.md for the complete procedure."
}
