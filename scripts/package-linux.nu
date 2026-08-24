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

def capture-checked [program: string, ...args: string] {
    let result = (run-external $program ...$args | complete)
    if $result.exit_code != 0 {
        if not ($result.stderr | is-empty) {
            print --stderr ($result.stderr | str trim)
        }
        error make { msg: $"command failed with exit code ($result.exit_code): ($program)" }
    }
    $result.stdout | str trim
}

def require-command [name: string] {
    if (which $name | is-empty) {
        fail $"Required tool is not installed: ($name)"
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

def install-payload [repo_root: string, binary: string, payload_root: string, prefix: string] {
    let root = if ($prefix | is-empty) {
        $payload_root
    } else {
        $payload_root | path join $prefix
    }
    run-checked install "-Dm0755" $binary ($root | path join "bin" "germinal")
    run-checked install "-Dm0644" ($repo_root | path join "packaging" "linux" "io.github.cradiy.Germinal.desktop") ($root | path join "share" "applications" "io.github.cradiy.Germinal.desktop")
    run-checked install "-Dm0644" ($repo_root | path join "packaging" "linux" "io.github.cradiy.Germinal.svg") ($root | path join "share" "icons" "hicolor" "scalable" "apps" "io.github.cradiy.Germinal.svg")
    run-checked install "-Dm0644" ($repo_root | path join "LICENSE") ($root | path join "share" "licenses" "germinal" "LICENSE")
    run-checked install "-Dm0644" ($repo_root | path join "README.md") ($root | path join "share" "doc" "germinal" "README.md")
    run-checked install "-Dm0644" ($repo_root | path join "packaging" "linux" "PACKAGE-README.md") ($root | path join "share" "doc" "germinal" "PACKAGE-README.md")
}

def write-checksum [artifact: string] {
    let directory = ($artifact | path dirname)
    let name = ($artifact | path basename)
    let checksum = (do { cd $directory; capture-checked sha256sum $name })
    $checksum | save --force $"($artifact).sha256"
    print $"Created ($artifact)"
    print $"Created ($artifact).sha256"
}

def package-tarball [
    repo_root: string,
    binary: string,
    temp_root: string,
    output_dir: string,
    version: string,
    architecture: string,
    target_libc: string,
] {
    require-command tar
    require-command gzip
    let platform = if $target_libc == "musl" { "linux-musl" } else { "linux" }
    let package_name = $"germinal-($version)-($platform)-($architecture)"
    let package_root = ($temp_root | path join $package_name)
    let artifact = ($output_dir | path join $"($package_name).tar.gz")
    install-payload $repo_root $binary $package_root ""
    run-checked tar "-C" $temp_root "-czf" $artifact $package_name
    write-checksum $artifact
}

def debian-architecture [architecture: string] {
    match $architecture {
        "x86_64" => "amd64"
        "aarch64" => "arm64"
        _ => $architecture
    }
}

def package-deb [
    repo_root: string,
    binary: string,
    temp_root: string,
    output_dir: string,
    version: string,
    architecture: string,
] {
    require-command dpkg-deb
    let deb_root = ($temp_root | path join "deb-root")
    let deb_arch = (debian-architecture $architecture)
    let artifact = ($output_dir | path join $"germinal_($version)_($deb_arch).deb")
    install-payload $repo_root $binary $deb_root "usr"
    let installed_size = (
        capture-checked du "-sk" ($deb_root | path join "usr")
        | split row --regex '\s+'
        | first
    )
    mkdir ($deb_root | path join "DEBIAN")
    $"Package: germinal
Version: ($version)
Section: utils
Priority: optional
Architecture: ($deb_arch)
Installed-Size: ($installed_size)
Maintainer: Germinal maintainers
Depends: libfontconfig1, libfreetype6, libglib2.0-0, libgstreamer1.0-0, libgstreamer-plugins-base1.0-0
Homepage: https://github.com/cradiy/germinal
Description: GPU-rendered terminal and structured UI host
 Germinal is a keyboard-first GPU-rendered terminal that combines PTY shell
 compatibility with structured UI applications.
" | save --force ($deb_root | path join "DEBIAN" "control")
    run-checked dpkg-deb "--root-owner-group" "--build" $deb_root $artifact
    write-checksum $artifact
}

def package-rpm [
    repo_root: string,
    binary: string,
    temp_root: string,
    output_dir: string,
    version: string,
] {
    require-command rpmbuild
    let rpm_top = ($temp_root | path join "rpmbuild")
    for directory in ["BUILD", "BUILDROOT", "RPMS", "SOURCES", "SPECS", "SRPMS"] {
        mkdir ($rpm_top | path join $directory)
    }
    let sources = ($rpm_top | path join "SOURCES")
    run-checked install "-m0755" $binary ($sources | path join "germinal")
    for source in [
        "io.github.cradiy.Germinal.desktop",
        "io.github.cradiy.Germinal.svg",
        "PACKAGE-README.md",
    ] {
        run-checked install "-m0644" ($repo_root | path join "packaging" "linux" $source) ($sources | path join $source)
    }
    for source in ["LICENSE", "README.md"] {
        run-checked install "-m0644" ($repo_root | path join $source) ($sources | path join $source)
    }

    let spec = ($rpm_top | path join "SPECS" "germinal.spec")
    open --raw ($repo_root | path join "packaging" "linux" "germinal.spec.in")
    | str replace --all "@VERSION@" $version
    | save --force $spec
    run-checked rpmbuild "--define" $"_topdir ($rpm_top)" "--define" $"_tmppath ($temp_root)" "-bb" $spec

    let built_rpm = (
        glob ($rpm_top | path join "RPMS" "**" "germinal-*.rpm")
        | first
    )
    let artifact = ($output_dir | path join ($built_rpm | path basename))
    run-checked install "-m0644" $built_rpm $artifact
    write-checksum $artifact
}

def cleanup [temp_root: string] {
    if ($temp_root | path exists) and (($temp_root | path basename) starts-with "germinal-package.") {
        rm --recursive --force $temp_root
    }
}

# Build and package Germinal for Linux.
def main [
    --format: string = "tar.gz"  # tar.gz, deb, rpm, or all.
    --output-dir: string          # Output directory; defaults to ./dist.
    --skip-build                  # Package an existing product binary.
    --target: string              # Package a specific Linux Rust target.
    --libc: string                # Select gnu or musl for the current architecture.
] {
    if ($target != null) and ($libc != null) {
        fail "--target and --libc cannot be used together" 2
    }
    if $format not-in ["tar.gz", "deb", "rpm", "all"] {
        fail $"Unsupported package format: ($format)" 2
    }
    if (capture-checked uname "-s") != "Linux" {
        fail "package-linux.nu can only package Linux binaries"
    }
    for tool in ["cargo", "install", "rustc", "sha256sum"] {
        require-command $tool
    }

    let repo_root = ($env.FILE_PWD | path dirname)
    cd $repo_root
    let version = (open Cargo.toml | get workspace.package.version)
    let host_target = (rust-host-target)
    let target_explicit = ($target != null) or ($libc != null)
    let selected_target = if $libc != null {
        target-for-libc $libc
    } else if $target != null {
        $target
    } else {
        $host_target
    }
    if not ($selected_target | str contains "-linux-") {
        fail $"package-linux.nu requires a Linux Rust target: ($selected_target)"
    }
    let architecture = ($selected_target | split row "-" | first)
    let target_libc = if ($selected_target | str contains "-musl") { "musl" } else { "gnu" }
    if ($target_libc == "musl") and ($format != "tar.gz") {
        fail "musl builds are distributed as tar.gz; DEB and RPM target glibc systems" 2
    }

    let configured_target_dir = ($env.CARGO_TARGET_DIR? | default ($repo_root | path join "target"))
    let target_dir = ($configured_target_dir | path expand)
    mut binary = if $target_explicit {
        $target_dir | path join $selected_target "product" "germinal"
    } else {
        $target_dir | path join "product" "germinal"
    }

    if not $skip_build {
        mut build_args = []
        if $target_explicit {
            $build_args = ($build_args | append ["--target", $selected_target])
        }
        $binary = (capture-checked nu ($repo_root | path join "scripts" "build-release.nu") ...$build_args)
    }
    if not ($binary | path exists) {
        fail $"Product binary is missing: ($binary)"
    }

    let output = if $output_dir == null {
        $repo_root | path join "dist"
    } else {
        $output_dir | path expand
    }
    mkdir $output
    let normalized_output = ($output | path expand)
    let temp_root = (capture-checked mktemp "-d" $"(($env.TMPDIR? | default '/tmp') | path join 'germinal-package.XXXXXX')")

    try {
        match $format {
            "tar.gz" => {
                package-tarball $repo_root $binary $temp_root $normalized_output $version $architecture $target_libc
            }
            "deb" => {
                package-deb $repo_root $binary $temp_root $normalized_output $version $architecture
            }
            "rpm" => {
                package-rpm $repo_root $binary $temp_root $normalized_output $version
            }
            "all" => {
                package-tarball $repo_root $binary $temp_root $normalized_output $version $architecture $target_libc
                package-deb $repo_root $binary $temp_root $normalized_output $version $architecture
                package-rpm $repo_root $binary $temp_root $normalized_output $version
            }
        }
    } catch {|error|
        cleanup $temp_root
        error make { msg: $error.msg }
    }
    cleanup $temp_root
}
