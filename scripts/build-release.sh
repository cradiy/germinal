#!/usr/bin/env bash

set -Eeuo pipefail

germinal_repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
germinal_target=
germinal_libc=

usage() {
    cat <<'EOF'
Usage: scripts/build-release.sh [options]

Build the Germinal desktop binary with the reproducible Cargo.lock and the
workspace's optimized `product` profile.

Options:
  --target TARGET
                 Build a specific Rust target, such as x86_64-unknown-linux-musl.
  --libc LIBC    Select the current Linux architecture with gnu or musl libc.
  -h, --help     Show this help.
EOF
}

while (($# > 0)); do
    case "$1" in
        --target)
            if (($# < 2)); then
                printf '%s\n' '--target requires a value' >&2
                exit 2
            fi
            germinal_target=$2
            shift
            ;;
        --libc)
            if (($# < 2)); then
                printf '%s\n' '--libc requires a value' >&2
                exit 2
            fi
            germinal_libc=$2
            shift
            ;;
        -h | --help)
            usage
            exit 0
            ;;
        *)
            printf 'Unknown argument: %s\n' "$1" >&2
            usage >&2
            exit 2
            ;;
    esac
    shift
done

if [[ -n "$germinal_target" && -n "$germinal_libc" ]]; then
    printf '%s\n' '--target and --libc cannot be used together' >&2
    exit 2
fi

if [[ -n "$germinal_libc" ]]; then
    case "$germinal_libc" in
        gnu | musl) ;;
        *)
            printf 'Unsupported libc: %s\n' "$germinal_libc" >&2
            exit 2
            ;;
    esac
    germinal_host_target=$(rustc -vV | sed -n 's/^host: //p')
    germinal_host_arch=${germinal_host_target%%-*}
    case "$germinal_host_arch" in
        x86_64 | aarch64) ;;
        *)
            printf 'Cannot derive a Linux %s target for architecture: %s\n' \
                "$germinal_libc" "$germinal_host_arch" >&2
            exit 1
            ;;
    esac
    germinal_target="$germinal_host_arch-unknown-linux-$germinal_libc"
fi

if [[ "$germinal_target" == *-musl* ]] && command -v rustup >/dev/null 2>&1; then
    germinal_target_installed=0
    while IFS= read -r germinal_installed_target; do
        if [[ "$germinal_installed_target" == "$germinal_target" ]]; then
            germinal_target_installed=1
            break
        fi
    done < <(rustup target list --installed)
    if ((!germinal_target_installed)); then
        printf 'Rust target is not installed: %s\n' "$germinal_target" >&2
        printf 'Install it with: rustup target add %s\n' "$germinal_target" >&2
        exit 1
    fi
fi

cd -- "$germinal_repo_root"

germinal_target_args=()
if [[ -n "$germinal_target" ]]; then
    germinal_target_args+=(--target "$germinal_target")
fi
cargo build --locked --profile product -p germinal "${germinal_target_args[@]}"

germinal_target_dir=${CARGO_TARGET_DIR:-"$germinal_repo_root/target"}
if [[ "$germinal_target_dir" != /* ]]; then
    germinal_target_dir="$germinal_repo_root/$germinal_target_dir"
fi
if [[ -n "$germinal_target" ]]; then
    germinal_binary="$germinal_target_dir/$germinal_target/product/germinal"
else
    germinal_binary="$germinal_target_dir/product/germinal"
fi

if [[ ! -x "$germinal_binary" ]]; then
    printf 'Built binary is missing or not executable: %s\n' "$germinal_binary" >&2
    exit 1
fi

printf '%s\n' "$germinal_binary"
