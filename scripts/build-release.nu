#!/usr/bin/env nu

def fail [message: string, code: int = 1] {
    print --stderr $message
    exit $code
}

def run-checked [program: string, ...args: string] {
    run-external $program ...$args
    let code = $env.LAST_EXIT_CODE
    if $code != 0 {
        error make { msg: $"command failed with exit code ($code): ($program)" }
    }
}

def rust-host-target [] {
    let target = (
        run-external rustc "-vV"
        | lines
        | where {|line| $line starts-with "host: " }
        | first
        | str replace "host: " ""
    )
    if ($target | is-empty) {
        fail "Could not determine the Rust host target"
    }
    $target
}

def target-for-libc [libc: string] {
    if $libc not-in ["gnu", "musl"] {
        fail $"Unsupported libc: ($libc)" 2
    }

    let architecture = (rust-host-target | split row "-" | first)
    if $architecture not-in ["x86_64", "aarch64"] {
        fail $"Cannot derive a Linux ($libc) target for architecture: ($architecture)"
    }
    $"($architecture)-unknown-linux-($libc)"
}

def ensure-musl-target [target: string] {
    if ($target | str contains "-musl") and ((which rustup | is-not-empty)) {
        let installed = (run-external rustup target list "--installed" | lines)
        if $target not-in $installed {
            print --stderr $"Rust target is not installed: ($target)"
            fail $"Install it with: rustup target add ($target)"
        }
    }
}

# Build Germinal with the optimized product profile.
def main [
    --target: string       # Build a specific Rust target.
    --libc: string         # Select gnu or musl for the current Linux architecture.
] {
    if ($target != null) and ($libc != null) {
        fail "--target and --libc cannot be used together" 2
    }

    let selected_target = if $libc != null {
        target-for-libc $libc
    } else {
        $target
    }
    if $selected_target != null {
        ensure-musl-target $selected_target
    }

    let repo_root = ($env.FILE_PWD | path dirname)
    cd $repo_root

    mut build_args = ["build", "--locked", "--profile", "product", "-p", "germinal"]
    if $selected_target != null {
        $build_args = ($build_args | append ["--target", $selected_target])
    }
    run-checked cargo ...$build_args

    let configured_target_dir = ($env.CARGO_TARGET_DIR? | default ($repo_root | path join "target"))
    let target_dir = ($configured_target_dir | path expand)
    let binary = if $selected_target == null {
        $target_dir | path join "product" "germinal"
    } else {
        $target_dir | path join $selected_target "product" "germinal"
    }

    if not ($binary | path exists) {
        fail $"Built binary is missing: ($binary)"
    }
    print $binary
}
