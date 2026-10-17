#!/usr/bin/env nu

use windows.nu [preflight-windows package-windows]

# Build the optimized native executable.
def main [] {
    nu ($env.FILE_PWD | path join build-release.nu)
    if $env.LAST_EXIT_CODE != 0 { error make {msg: "Build failed"} }
}

# Build Windows portable ZIP and installer, or selected formats.
def "main windows" [
    ...formats: string # zip, setup; omitted means both
    --skip-build # Package an existing product binary
    --output: path # Defaults to dist/
] {
    if $nu.os-info.name != "windows" { error make {msg: "Run Windows packaging on Windows."} }
    for format in $formats {
        if $format not-in [zip setup] { error make {msg: $"Unknown Windows format: ($format)"} }
    }
    let selected = if ($formats | is-empty) { [zip setup] } else { $formats | uniq }
    preflight-windows $selected
    let root = ($env.FILE_PWD | path dirname)
    let destination = if $output == null { $root | path join dist } else { $output | path expand }
    cd $root
    let host_result = (rustc -vV | complete)
    if $host_result.exit_code != 0 { error make {msg: $host_result.stderr} }
    let host = ($host_result.stdout | lines | parse 'host: {host}' | get host | first)
    if $host != "x86_64-pc-windows-msvc" {
        error make {msg: "Windows packages require the x86_64-pc-windows-msvc toolchain."}
    }
    let result = (cargo metadata --locked --no-deps --format-version 1 | complete)
    if $result.exit_code != 0 { error make {msg: $result.stderr} }
    let metadata = ($result.stdout | from json)
    let package = ($metadata.packages | where name == germinal | first)
    if not $skip_build {
        cargo build --locked --profile product -p germinal --bin germinal --target $host
        if $env.LAST_EXIT_CODE != 0 { error make {msg: "Windows build failed"} }
    }
    let binary = ($metadata.target_directory | path join $host product germinal.exe)
    if not ($binary | path exists) { error make {msg: $"Missing ($binary). Run without --skip-build first."} }
    package-windows {
        root: $root, binary: $binary, arch: "x86_64", version: $package.version,
        profile: "product", target_directory: $metadata.target_directory
    } $selected $destination
}
