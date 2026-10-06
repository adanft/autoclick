#!/usr/bin/env bash
# Builds autoclick in release mode and installs the binary system-wide.
#
# Usage:
#   ./install.sh              build and install to $BIN_DIR (default /usr/local/bin)
#   ./install.sh --uninstall  remove the installed binary
#
# Run it as your normal user, or with sudo: either way cargo builds as your
# user, never as root, and only the copy into $BIN_DIR runs with root rights.

set -euo pipefail

BIN_DIR="${BIN_DIR:-/usr/local/bin}"
BINARY_NAME="autoclick"
TARGET="${BIN_DIR}/${BINARY_NAME}"

die() {
    echo "error: $*" >&2
    exit 1
}

# Building as root would leave root-owned files in target/ that break later
# builds, so the build always runs as the invoking user. Under sudo that is
# $SUDO_USER, and the install step needs no further sudo.
if [[ "${EUID}" -eq 0 ]]; then
    if [[ -z "${SUDO_USER:-}" || "${SUDO_USER}" == "root" ]]; then
        die "run this script as your normal user (with or without sudo), not from a root login"
    fi
    as_root=()
    as_builder=(sudo -u "${SUDO_USER}" -H --)
else
    command -v sudo >/dev/null 2>&1 || die "sudo is required to write to ${BIN_DIR}"
    as_root=(sudo)
    as_builder=()
fi

case "${1:-}" in
    "")
        ;;
    --uninstall)
        if [[ -e "${TARGET}" ]]; then
            "${as_root[@]}" rm -f -- "${TARGET}"
            echo "removed ${TARGET}"
        else
            echo "nothing to remove: ${TARGET} does not exist"
        fi
        exit 0
        ;;
    -h | --help)
        sed -n '2,9p' "$0" | sed 's/^# \{0,1\}//'
        exit 0
        ;;
    *)
        die "unknown argument: $1 (use --uninstall or --help)"
        ;;
esac

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# Under sudo, PATH is root's, so look for the invoking user's cargo: rustup's
# default location first, so a rustup toolchain wins over a system one, then
# their login shell PATH.
if [[ "${EUID}" -eq 0 ]]; then
    builder_home="$(getent passwd "${SUDO_USER}" | cut -d: -f6)"
    if [[ -x "${builder_home}/.cargo/bin/cargo" ]]; then
        cargo_bin="${builder_home}/.cargo/bin/cargo"
    else
        cargo_bin="$("${as_builder[@]}" bash -lc 'command -v cargo' 2>/dev/null || true)"
    fi
else
    cargo_bin="$(command -v cargo || true)"
fi
[[ -n "${cargo_bin}" ]] || die "cargo is not installed or not in PATH"

echo "building ${BINARY_NAME} (release) as ${SUDO_USER:-${USER}}..."
"${as_builder[@]}" "${cargo_bin}" build --release --locked

echo "installing to ${TARGET}..."
"${as_root[@]}" install -Dm755 -- "target/release/${BINARY_NAME}" "${TARGET}"

echo "installed ${TARGET}"

resolved="$(command -v "${BINARY_NAME}" || true)"
if [[ -z "${resolved}" ]]; then
    echo "warning: ${BIN_DIR} is not in PATH; run ${TARGET} directly or add it to PATH" >&2
elif [[ "${resolved}" != "${TARGET}" ]]; then
    echo "warning: '${BINARY_NAME}' resolves to ${resolved}, which comes before ${TARGET} in PATH" >&2
fi

if ! command -v hyprctl >/dev/null 2>&1; then
    echo "warning: hyprctl is not in PATH; autoclick needs it at runtime" >&2
fi
