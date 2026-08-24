#!/usr/bin/env bash

set -Eeuo pipefail

germinal_repo_root=$(cd -- "$(dirname -- "${BASH_SOURCE[0]}")/.." && pwd)
germinal_format=tar.gz
germinal_output_dir="$germinal_repo_root/dist"
germinal_skip_build=0
germinal_temp_root=
germinal_target=
germinal_libc=

usage() {
    cat <<'EOF'
Usage: scripts/package-linux.sh [options]

Build and package Germinal for the current Linux architecture.

Options:
  --format FORMAT  tar.gz (default), deb, rpm, arch, or all.
  --output-dir DIR Write packages to DIR instead of ./dist.
  --skip-build     Package an existing target/product/germinal binary.
  --target TARGET  Package a specific Linux Rust target.
  --libc LIBC      Select the current architecture with gnu or musl libc.
  -h, --help       Show this help.
EOF
}

while (($# > 0)); do
    case "$1" in
        --format)
            if (($# < 2)); then
                printf '%s\n' '--format requires a value' >&2
                exit 2
            fi
            germinal_format=$2
            shift
            ;;
        --output-dir)
            if (($# < 2)); then
                printf '%s\n' '--output-dir requires a value' >&2
                exit 2
            fi
            germinal_output_dir=$2
            shift
            ;;
        --skip-build)
            germinal_skip_build=1
            ;;
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

germinal_target_explicit=0
if [[ -n "$germinal_target" || -n "$germinal_libc" ]]; then
    germinal_target_explicit=1
fi

case "$germinal_format" in
    tar.gz | deb | rpm | arch | all) ;;
    *)
        printf 'Unsupported package format: %s\n' "$germinal_format" >&2
        exit 2
        ;;
esac

if [[ $(uname -s) != Linux ]]; then
    printf '%s\n' 'package-linux.sh can only package Linux binaries' >&2
    exit 1
fi

for germinal_tool in cargo install rustc sha256sum; do
    if ! command -v "$germinal_tool" >/dev/null 2>&1; then
        printf 'Required tool is not installed: %s\n' "$germinal_tool" >&2
        exit 1
    fi
done

cd -- "$germinal_repo_root"

germinal_version=$(awk '
    /^\[workspace\.package\]$/ { in_workspace_package = 1; next }
    in_workspace_package && /^\[/ { exit }
    in_workspace_package && /^version = / {
        gsub(/^version = "/, "")
        gsub(/"$/, "")
        print
        exit
    }
' Cargo.toml)
if [[ -z "$germinal_version" ]]; then
    printf '%s\n' 'Could not read workspace.package.version from Cargo.toml' >&2
    exit 1
fi

germinal_host_target=$(rustc -vV | sed -n 's/^host: //p')
if [[ -z "$germinal_host_target" ]]; then
    printf '%s\n' 'Could not determine the Rust host target' >&2
    exit 1
fi
if [[ -n "$germinal_libc" ]]; then
    case "$germinal_libc" in
        gnu | musl) ;;
        *)
            printf 'Unsupported libc: %s\n' "$germinal_libc" >&2
            exit 2
            ;;
    esac
    germinal_arch=${germinal_host_target%%-*}
    case "$germinal_arch" in
        x86_64 | aarch64) ;;
        *)
            printf 'Cannot derive a Linux %s target for architecture: %s\n' \
                "$germinal_libc" "$germinal_arch" >&2
            exit 1
            ;;
    esac
    germinal_target="$germinal_arch-unknown-linux-$germinal_libc"
elif [[ -z "$germinal_target" ]]; then
    germinal_target=$germinal_host_target
fi
if [[ "$germinal_target" != *-linux-* ]]; then
    printf 'package-linux.sh requires a Linux Rust target: %s\n' "$germinal_target" >&2
    exit 1
fi
germinal_arch=${germinal_target%%-*}
case "$germinal_target" in
    *-musl*) germinal_target_libc=musl ;;
    *) germinal_target_libc=gnu ;;
esac

if [[ "$germinal_target_libc" == musl && "$germinal_format" != tar.gz ]]; then
    printf '%s\n' 'musl builds are distributed as tar.gz; DEB, RPM, and Arch packages target glibc systems' >&2
    exit 2
fi

germinal_target_dir=${CARGO_TARGET_DIR:-"$germinal_repo_root/target"}
if [[ "$germinal_target_dir" != /* ]]; then
    germinal_target_dir="$germinal_repo_root/$germinal_target_dir"
fi
if ((germinal_target_explicit)); then
    germinal_binary="$germinal_target_dir/$germinal_target/product/germinal"
else
    germinal_binary="$germinal_target_dir/product/germinal"
fi

if ((!germinal_skip_build)); then
    germinal_build_args=()
    if ((germinal_target_explicit)); then
        germinal_build_args+=(--target "$germinal_target")
    fi
    germinal_binary=$("$germinal_repo_root/scripts/build-release.sh" "${germinal_build_args[@]}")
fi

if [[ ! -f "$germinal_binary" || ! -x "$germinal_binary" ]]; then
    printf 'Product binary is missing or not executable: %s\n' "$germinal_binary" >&2
    exit 1
fi

mkdir -p -- "$germinal_output_dir"
germinal_output_dir=$(cd -- "$germinal_output_dir" && pwd)
germinal_temp_root=$(mktemp -d "${TMPDIR:-/tmp}/germinal-package.XXXXXX")

cleanup() {
    if [[ -n "$germinal_temp_root" && -d "$germinal_temp_root" && "$germinal_temp_root" == */germinal-package.* ]]; then
        rm -rf -- "$germinal_temp_root"
    fi
}
trap cleanup EXIT

install_payload() {
    local germinal_payload_root=$1
    local germinal_prefix=$2

    install -Dm0755 "$germinal_binary" "$germinal_payload_root$germinal_prefix/bin/germinal"
    install -Dm0644 \
        "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.desktop" \
        "$germinal_payload_root$germinal_prefix/share/applications/io.github.cradiy.Germinal.desktop"
    install -Dm0644 \
        "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.svg" \
        "$germinal_payload_root$germinal_prefix/share/icons/hicolor/scalable/apps/io.github.cradiy.Germinal.svg"
    install -Dm0644 "$germinal_repo_root/LICENSE" \
        "$germinal_payload_root$germinal_prefix/share/licenses/germinal/LICENSE"
    install -Dm0644 "$germinal_repo_root/README.md" \
        "$germinal_payload_root$germinal_prefix/share/doc/germinal/README.md"
    install -Dm0644 "$germinal_repo_root/packaging/linux/PACKAGE-README.md" \
        "$germinal_payload_root$germinal_prefix/share/doc/germinal/PACKAGE-README.md"
}

write_checksum() {
    local germinal_artifact=$1
    local germinal_artifact_dir
    local germinal_artifact_name
    germinal_artifact_dir=$(dirname -- "$germinal_artifact")
    germinal_artifact_name=$(basename -- "$germinal_artifact")
    (
        cd -- "$germinal_artifact_dir"
        sha256sum "$germinal_artifact_name" >"$germinal_artifact_name.sha256"
    )
    printf 'Created %s\n' "$germinal_artifact"
    printf 'Created %s.sha256\n' "$germinal_artifact"
}

package_tarball() {
    for germinal_tool in tar gzip; do
        if ! command -v "$germinal_tool" >/dev/null 2>&1; then
            printf 'Required tool is not installed: %s\n' "$germinal_tool" >&2
            exit 1
        fi
    done

    local germinal_platform="linux"
    if [[ "$germinal_target_libc" == musl ]]; then
        germinal_platform=linux-musl
    fi
    local germinal_package_name="germinal-$germinal_version-$germinal_platform-$germinal_arch"
    local germinal_package_root="$germinal_temp_root/$germinal_package_name"
    local germinal_artifact="$germinal_output_dir/$germinal_package_name.tar.gz"
    install_payload "$germinal_package_root" ""
    tar -C "$germinal_temp_root" -czf "$germinal_artifact" "$germinal_package_name"
    write_checksum "$germinal_artifact"
}

debian_architecture() {
    case "$germinal_arch" in
        x86_64) printf '%s\n' amd64 ;;
        aarch64) printf '%s\n' arm64 ;;
        *) printf '%s\n' "$germinal_arch" ;;
    esac
}

package_deb() {
    if ! command -v dpkg-deb >/dev/null 2>&1; then
        printf '%s\n' 'dpkg-deb is required for --format deb' >&2
        exit 1
    fi

    local germinal_deb_root="$germinal_temp_root/deb-root"
    local germinal_deb_arch
    local germinal_installed_size
    local germinal_artifact
    germinal_deb_arch=$(debian_architecture)
    germinal_artifact="$germinal_output_dir/germinal_${germinal_version}_${germinal_deb_arch}.deb"
    install_payload "$germinal_deb_root" /usr
    germinal_installed_size=$(du -sk "$germinal_deb_root/usr" | awk '{ print $1 }')
    mkdir -p "$germinal_deb_root/DEBIAN"
    cat >"$germinal_deb_root/DEBIAN/control" <<EOF
Package: germinal
Version: $germinal_version
Section: utils
Priority: optional
Architecture: $germinal_deb_arch
Installed-Size: $germinal_installed_size
Maintainer: Germinal maintainers
Depends: libfontconfig1, libfreetype6, libglib2.0-0, libgstreamer1.0-0, libgstreamer-plugins-base1.0-0
Homepage: https://github.com/cradiy/germinal
Description: GPU-rendered terminal and structured UI host
 Germinal is a keyboard-first GPU-rendered terminal that combines PTY shell
 compatibility with structured UI applications.
EOF
    dpkg-deb --root-owner-group --build "$germinal_deb_root" "$germinal_artifact"
    write_checksum "$germinal_artifact"
}

package_rpm() {
    if ! command -v rpmbuild >/dev/null 2>&1; then
        printf '%s\n' 'rpmbuild is required for --format rpm' >&2
        exit 1
    fi

    local germinal_rpm_top="$germinal_temp_root/rpmbuild"
    local germinal_spec="$germinal_rpm_top/SPECS/germinal.spec"
    mkdir -p "$germinal_rpm_top"/{BUILD,BUILDROOT,RPMS,SOURCES,SPECS,SRPMS}
    install -m0755 "$germinal_binary" "$germinal_rpm_top/SOURCES/germinal"
    install -m0644 "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.desktop" \
        "$germinal_rpm_top/SOURCES/io.github.cradiy.Germinal.desktop"
    install -m0644 "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.svg" \
        "$germinal_rpm_top/SOURCES/io.github.cradiy.Germinal.svg"
    install -m0644 "$germinal_repo_root/LICENSE" "$germinal_rpm_top/SOURCES/LICENSE"
    install -m0644 "$germinal_repo_root/README.md" "$germinal_rpm_top/SOURCES/README.md"
    install -m0644 "$germinal_repo_root/packaging/linux/PACKAGE-README.md" \
        "$germinal_rpm_top/SOURCES/PACKAGE-README.md"
    sed "s/@VERSION@/$germinal_version/g" \
        "$germinal_repo_root/packaging/linux/germinal.spec.in" >"$germinal_spec"
    rpmbuild \
        --define "_topdir $germinal_rpm_top" \
        --define "_tmppath $germinal_temp_root" \
        -bb "$germinal_spec"

    local germinal_built_rpm
    local germinal_artifact
    germinal_built_rpm=$(find "$germinal_rpm_top/RPMS" -type f -name 'germinal-*.rpm' -print -quit)
    if [[ -z "$germinal_built_rpm" ]]; then
        printf '%s\n' 'rpmbuild completed without producing a Germinal RPM' >&2
        exit 1
    fi
    germinal_artifact="$germinal_output_dir/$(basename -- "$germinal_built_rpm")"
    install -m0644 "$germinal_built_rpm" "$germinal_artifact"
    write_checksum "$germinal_artifact"
}

arch_architecture() {
    case "$germinal_arch" in
        x86_64) printf '%s\n' x86_64 ;;
        aarch64) printf '%s\n' aarch64 ;;
        *)
            printf 'Unsupported Arch Linux package architecture: %s\n' "$germinal_arch" >&2
            exit 1
            ;;
    esac
}

package_arch() {
    if ! command -v makepkg >/dev/null 2>&1; then
        printf '%s\n' 'makepkg is required for --format arch' >&2
        exit 1
    fi

    local germinal_arch_root="$germinal_temp_root/arch-package"
    local germinal_arch_name
    local germinal_pkgbuild="$germinal_arch_root/PKGBUILD"
    germinal_arch_name=$(arch_architecture)
    mkdir -p "$germinal_arch_root"
    install -m0755 "$germinal_binary" "$germinal_arch_root/germinal"
    install -m0644 "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.desktop" \
        "$germinal_arch_root/io.github.cradiy.Germinal.desktop"
    install -m0644 "$germinal_repo_root/packaging/linux/io.github.cradiy.Germinal.svg" \
        "$germinal_arch_root/io.github.cradiy.Germinal.svg"
    install -m0644 "$germinal_repo_root/LICENSE" "$germinal_arch_root/LICENSE"
    install -m0644 "$germinal_repo_root/README.md" "$germinal_arch_root/README.md"
    install -m0644 "$germinal_repo_root/packaging/linux/PACKAGE-README.md" \
        "$germinal_arch_root/PACKAGE-README.md"
    sed \
        -e "s/@VERSION@/$germinal_version/g" \
        -e "s/@ARCH@/$germinal_arch_name/g" \
        "$germinal_repo_root/packaging/linux/PKGBUILD.in" >"$germinal_pkgbuild"

    (
        cd -- "$germinal_arch_root"
        PKGDEST="$germinal_output_dir" makepkg --force --nodeps --noconfirm
    )

    local germinal_artifact
    germinal_artifact=$(
        cd -- "$germinal_arch_root"
        PKGDEST="$germinal_output_dir" makepkg --packagelist
    )
    germinal_artifact=${germinal_artifact%%$'\n'*}
    if [[ -z "$germinal_artifact" || ! -f "$germinal_artifact" ]]; then
        printf '%s\n' 'makepkg completed without producing a Germinal package' >&2
        exit 1
    fi
    write_checksum "$germinal_artifact"
}

case "$germinal_format" in
    tar.gz) package_tarball ;;
    deb) package_deb ;;
    rpm) package_rpm ;;
    arch) package_arch ;;
    all)
        package_tarball
        package_deb
        package_rpm
        package_arch
        ;;
esac
