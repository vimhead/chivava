#!/usr/bin/env sh
set -eu

die() {
    printf 'chivava install: %s\n' "$*" >&2
    exit 1
}

command_exists() {
    command -v "$1" >/dev/null 2>&1
}

detect_asset_name() {
    os=$(uname -s)
    machine=$(uname -m)
    case "$os" in
        Darwin) platform=darwin ;;
        Linux) platform=linux ;;
        MINGW*|MSYS*|CYGWIN*) platform=windows ;;
        *) die "unsupported operating system: $os" ;;
    esac
    case "$machine" in
        x86_64|amd64) arch=x64 ;;
        aarch64|arm64) arch=arm64 ;;
        *) die "unsupported architecture: $machine" ;;
    esac
    if [ "$platform" = darwin ] && [ "$arch" = x64 ] && command_exists sysctl; then
        if [ "$(sysctl -in sysctl.proc_translated 2>/dev/null || true)" = 1 ]; then
            arch=arm64
        fi
    fi
    if [ "$platform" = windows ]; then
        [ "$arch" = x64 ] || die "Windows ARM64 builds are not available"
        printf 'chivava-windows-x64.exe'
    else
        printf 'chivava-%s-%s' "$platform" "$arch"
    fi
}

download_file() {
    if command_exists curl; then
        curl --proto '=https' --tlsv1.2 -fsSL --retry 3 --connect-timeout 15 "$1" -o "$2"
    elif command_exists wget; then
        wget --https-only -qO "$2" "$1"
    else
        die "curl or wget is required"
    fi
}

calculate_checksum() {
    if command_exists sha256sum; then
        sha256sum "$1" | awk '{print $1}'
    elif command_exists shasum; then
        shasum -a 256 "$1" | awk '{print $1}'
    else
        die "sha256sum or shasum is required"
    fi
}

verify_checksum() {
    expected=$(awk 'NR == 1 {print $1} END {if (NR != 1) exit 1}' "$2") || die "invalid checksum file"
    case "$expected" in
        ''|*[!0-9a-fA-F]*) die "invalid SHA-256 checksum" ;;
    esac
    [ "${#expected}" -eq 64 ] || die "invalid SHA-256 checksum length"
    expected=$(printf '%s' "$expected" | tr 'A-F' 'a-f')
    actual=$(calculate_checksum "$1") || die "could not calculate SHA-256 checksum"
    [ "$actual" = "$expected" ] || die "checksum mismatch; the release may be updating, please retry"
}

cleanup() {
    if [ -n "$temporary_target" ]; then rm -f -- "$temporary_target"; fi
    if [ -n "$temp_dir" ]; then rm -rf -- "$temp_dir"; fi
}

main() {
    [ "$#" -eq 0 ] || die "configure installation with CHIVAVA_INSTALL_DIR, not command-line arguments"
    repository=${CHIVAVA_REPOSITORY:-vimhead/chivava}
    release_tag=${CHIVAVA_RELEASE_TAG:-tip}
    base_url=${CHIVAVA_RELEASE_BASE_URL:-https://github.com/$repository/releases/download/$release_tag}
    case "$base_url" in
        https://*) ;;
        *) die "release URL must use HTTPS" ;;
    esac
    if [ -n "${CHIVAVA_INSTALL_DIR:-}" ]; then
        install_dir=$CHIVAVA_INSTALL_DIR
    else
        [ -n "${HOME:-}" ] || die "HOME is required when CHIVAVA_INSTALL_DIR is unset"
        install_dir=$HOME/.local/bin
    fi
    asset_name=$(detect_asset_name)
    case "$asset_name" in
        *.exe) binary_name=chivava.exe ;;
        *) binary_name=chivava ;;
    esac
    target_path=$install_dir/$binary_name
    [ ! -d "$target_path" ] || die "$target_path is a directory"
    temp_dir=
    temporary_target=
    trap cleanup 0
    trap 'exit 130' INT
    trap 'exit 143' TERM
    temp_dir=$(mktemp -d)
    binary_path=$temp_dir/$asset_name
    checksum_path=$temp_dir/$asset_name.sha256
    download_file "$base_url/$asset_name" "$binary_path" || die "could not download $asset_name"
    download_file "$base_url/$asset_name.sha256" "$checksum_path" || die "could not download checksum for $asset_name"
    verify_checksum "$binary_path" "$checksum_path"
    chmod 0755 "$binary_path"
    installed_version=$("$binary_path" --version) || die "downloaded binary cannot run; Linux requires glibc 2.35+ and ALSA (libasound2), macOS requires 11+"
    case "$installed_version" in
        'chivava '*) ;;
        *) die "downloaded file did not identify itself as chivava" ;;
    esac
    mkdir -p "$install_dir"
    temporary_target=$(mktemp "$install_dir/.chivava-install.XXXXXX")
    cp "$binary_path" "$temporary_target"
    chmod 0755 "$temporary_target"
    mv -f "$temporary_target" "$target_path"
    temporary_target=
    printf '%s installed to %s\n' "$installed_version" "$target_path"
    case ":${PATH:-}:" in
        *:"$install_dir":*) ;;
        *) printf 'Add %s to PATH if chivava is not found.\n' "$install_dir" ;;
    esac
}

main "$@"
