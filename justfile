set windows-shell := ["nu", "-c"]

# List build commands.
default:
    @just --list

# Build the optimized native executable.
build:
    nu scripts/build.nu

# Build Windows portable ZIP and installer.
build-windows:
    nu scripts/build.nu windows

build-windows-zip:
    nu scripts/build.nu windows zip

build-windows-setup:
    nu scripts/build.nu windows setup

build-linux:
    nu scripts/build-release.nu

build-linux-all:
    nu scripts/package-linux.nu --format all

build-linux-tar:
    nu scripts/package-linux.nu

build-linux-deb:
    nu scripts/package-linux.nu --format deb

build-linux-rpm:
    nu scripts/package-linux.nu --format rpm

build-linux-arch:
    nu scripts/package-linux.nu --format arch
