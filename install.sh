#!/bin/sh
# Install a verified hc-tui release. Works with both `sh install.sh` and curl | sh.
set -eu

fail() {
    printf 'hc-tui install: %s\n' "$*" >&2
    exit 1
}

cleanup() {
    if [ -n "$install_stage" ]; then rm -f "$install_stage"; fi
    if [ -n "$install_tmp" ]; then rm -rf "$install_tmp"; fi
}

fetch() {
    curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
        --fail --location --silent --show-error --connect-timeout 10 --max-time 180 \
        --output "$2" "$1" || fail "download failed: $1"
}

main() {
    repository='wuxiaobai24/hc-tui'
    release_version=''
    install_dir=${HC_TUI_INSTALL_DIR:-${HOME:-}/.local/bin}
    install_tmp=''
    install_stage=''
    while [ "$#" -gt 0 ]; do
        case "$1" in
            --version)
                [ "$#" -ge 2 ] || fail '--version needs a value'
                release_version=${2#v}
                [ -n "$release_version" ] || fail "--version needs a version"
                shift 2 ;;
            --bin-dir)
                [ "$#" -ge 2 ] || fail '--bin-dir needs a directory'
                install_dir=$2; shift 2 ;;
            --help|-h)
                printf '%s\n' 'Usage: install.sh [--version X.Y.Z] [--bin-dir DIRECTORY]' \
                    'Default: latest release, installed in ~/.local/bin.' \
                    'HC_TUI_INSTALL_DIR also selects the installation directory.'
                return ;;
            *) fail "unknown option: $1" ;;
        esac
    done
    [ -n "$install_dir" ] || fail 'installation directory is empty'
    if [ -z "${HOME:-}" ] && [ "$install_dir" = '/.local/bin' ]; then
        fail 'HOME is unset; specify --bin-dir'
    fi
    for dependency in curl tar awk mktemp uname chmod mv cp; do
        command -v "$dependency" >/dev/null 2>&1 || fail "missing dependency: $dependency"
    done
    if command -v sha256sum >/dev/null 2>&1; then
        hash_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        hash_tool=shasum
    else
        fail 'need sha256sum (Linux) or shasum (macOS)'
    fi
    system=$(uname -s)
    architecture=$(uname -m)
    case "$system" in
        Linux) platform=unknown-linux-musl ;;
        Darwin)
            platform=apple-darwin
            # Prefer the native executable when invoked through Rosetta on Apple Silicon.
            if [ "$(sysctl -n hw.optional.arm64 2>/dev/null || :)" = 1 ]; then
                architecture=arm64
            fi ;;
        *) fail "unsupported operating system: $system" ;;
    esac
    case "$architecture" in
        x86_64|amd64) architecture=x86_64 ;;
        aarch64|arm64) architecture=aarch64 ;;
        *) fail "unsupported architecture: $architecture" ;;
    esac
    target=$architecture-$platform
    if [ -z "$release_version" ]; then
        release_url=$(curl --proto '=https' --proto-redir '=https' --tlsv1.2 \
            --fail --location --silent --show-error --head --connect-timeout 10 --max-time 30 \
            --output /dev/null --write-out '%{url_effective}' \
            "https://github.com/$repository/releases/latest") || fail 'cannot resolve latest release'
        case "$release_url" in
            "https://github.com/$repository/releases/tag/v"*) release_version=${release_url#"https://github.com/$repository/releases/tag/v"} ;;
            *) fail "unexpected release URL: $release_url" ;;
        esac
    fi
    printf '%s\n' "$release_version" | LC_ALL=C awk '
        /^[0-9]+\.[0-9]+\.[0-9]+$/ { valid=1 }
        END { exit !(NR==1 && valid) }
    ' || fail 'version must look like 0.4.1'

    umask 077
    install_tmp=$(mktemp -d "${TMPDIR:-/tmp}/hc-tui-install.XXXXXX") || fail 'cannot create temporary directory'
    trap cleanup 0
    trap 'exit 1' 1 2 15
    archive="hc-tui-$release_version-$target.tar.gz"
    base="https://github.com/$repository/releases/download/v$release_version"
    printf 'Installing hc-tui %s for %s\n' "$release_version" "$target"
    fetch "$base/$archive" "$install_tmp/$archive"
    fetch "$base/$archive.sha256" "$install_tmp/$archive.sha256"
    expected=$(awk 'NR==1 {print $1}' "$install_tmp/$archive.sha256")
    [ "${#expected}" -eq 64 ] || fail 'invalid SHA-256 file'
    case "$expected" in *[!0-9a-fA-F]*) fail 'invalid SHA-256 file' ;; esac
    if [ "$hash_tool" = sha256sum ]; then
        actual=$(sha256sum "$install_tmp/$archive") || fail 'SHA-256 calculation failed'
    else
        actual=$(shasum -a 256 "$install_tmp/$archive") || fail 'SHA-256 calculation failed'
    fi
    actual=${actual%% *}
    [ "$actual" = "$expected" ] || fail 'SHA-256 mismatch; installation was not changed'
    tar -tzf "$install_tmp/$archive" > "$install_tmp/members" || fail 'invalid release archive'
    member=$(awk '$0=="./hc-tui" || $0=="hc-tui" {print; exit}' "$install_tmp/members")
    [ -n "$member" ] || fail 'archive has no hc-tui executable at its root'
    mkdir "$install_tmp/unpacked"
    tar -xzf "$install_tmp/$archive" -C "$install_tmp/unpacked" "$member" || fail 'cannot unpack executable'
    candidate="$install_tmp/unpacked/hc-tui"
    [ -f "$candidate" ] && [ ! -L "$candidate" ] || fail 'executable is not a regular file'
    actual_version=$("$candidate" --version) || fail 'executable cannot run on this machine'
    [ "$actual_version" = "hc-tui $release_version" ] || fail 'executable version does not match release'

    mkdir -p "$install_dir" || fail "cannot create $install_dir"
    install_dir=$(cd "$install_dir" && pwd -P)
    [ ! -d "$install_dir/hc-tui" ] || fail 'destination hc-tui is a directory'
    install_stage=$(mktemp "$install_dir/.hc-tui-install.XXXXXX") || fail "cannot write to $install_dir"
    cp "$candidate" "$install_stage"
    chmod 755 "$install_stage"
    mv -f "$install_stage" "$install_dir/hc-tui"
    install_stage=''
    printf 'Installed %s/hc-tui\n' "$install_dir"
    printf '%s\n' 'Upgrade later: hc-tui update'
    case ":${PATH:-}:" in
        *":$install_dir:"*) ;;
        *) printf 'Add this directory to PATH: %s\n' "$install_dir" ;;
    esac
    if ! command -v hc >/dev/null 2>&1; then
        printf '%s\n' 'Runtime prerequisite: install herdr-core (hc) before starting hc-tui.'
    fi
}

# Keep execution at the end so a truncated piped download cannot partially install.
main "$@"
