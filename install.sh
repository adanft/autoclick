#!/usr/bin/env bash
# Builds autoclick in release mode and installs the binary system-wide.
# Run ./install.sh --help for usage.

set -euo pipefail

BIN_DIR="${BIN_DIR:-/usr/local/bin}"
BINARY_NAME="autoclick"
TARGET="${BIN_DIR}/${BINARY_NAME}"

# Marks the line carrying the invoking user's PATH in a login shell's output,
# so banners or other text printed by their profile cannot be mistaken for it.
PATH_MARKER="__autoclick_user_path__="

# Upper bound for that login shell. It only reads profile files, so it answers
# in well under a second; the bound keeps a profile that blocks from hanging
# the install, which then falls back to the system PATH.
PATH_PROBE_TIMEOUT_SECONDS=10

usage() {
    cat <<EOF
Builds ${BINARY_NAME} in release mode and installs the binary system-wide.

Usage:
  ./install.sh              build and install to \$BIN_DIR (default /usr/local/bin)
  sudo ./install.sh         the same; the build still runs as you, not as root
  ./install.sh --uninstall  remove the installed binary
  ./install.sh --help       show this help

Set BIN_DIR to install elsewhere. With sudo, pass it after sudo so it survives
sudo's environment reset: sudo BIN_DIR=/usr/bin ./install.sh

The build always runs as your user, so target/ never gets root-owned files;
only the copy into \$BIN_DIR runs with root rights.
EOF
}

die() {
    echo "error: $*" >&2
    exit 1
}

[[ $# -le 1 ]] || die "too many arguments (use --uninstall or --help)"

mode="install"
case "${1:-}" in
    "") ;;
    --uninstall) mode="uninstall" ;;
    -h | --help)
        usage
        exit 0
        ;;
    *) die "unknown argument: $1 (use --uninstall or --help)" ;;
esac

# Under sudo, the invoking user is $SUDO_USER. A root login without sudo has no
# user to build as, so it may only uninstall.
under_sudo=false
if [[ "${EUID}" -eq 0 ]]; then
    if [[ -n "${SUDO_USER:-}" && "${SUDO_USER}" != "root" ]]; then
        under_sudo=true
    elif [[ "${mode}" == "install" ]]; then
        die "run this script as your normal user (with or without sudo), not from a root login"
    fi
else
    command -v sudo >/dev/null 2>&1 || die "sudo is required to write to ${BIN_DIR}"
fi

# Runs a command with root rights.
as_root() {
    if [[ "${EUID}" -eq 0 ]]; then
        "$@"
    else
        sudo "$@"
    fi
}

# Runs a command as the invoking user, never as root.
as_builder() {
    if [[ "${under_sudo}" == true ]]; then
        sudo -u "${SUDO_USER}" -H -- "$@"
    else
        "$@"
    fi
}

if [[ "${mode}" == "uninstall" ]]; then
    # -L also catches a dangling symlink, which -e alone reports as missing.
    if [[ -e "${TARGET}" || -L "${TARGET}" ]]; then
        as_root rm -f -- "${TARGET}"
        echo "removed ${TARGET}"
    else
        echo "nothing to remove: ${TARGET} does not exist"
    fi
    exit 0
fi

cd -- "$(dirname -- "${BASH_SOURCE[0]}")"

# The PATH the invoking user's own shell sees. Under sudo, PATH is root's
# secure_path, so ask their login shell instead. It runs non-interactively: an
# interactive shell under sudo and timeout can stop on terminal job control,
# so PATH set only in interactive rc files such as ~/.zshrc is not seen.
# Empty when that shell cannot report it.
if [[ "${under_sudo}" == true ]]; then
    builder_home="$(getent passwd "${SUDO_USER}" | cut -d: -f6)"
    builder_shell="$(getent passwd "${SUDO_USER}" | cut -d: -f7)"
    user_path="$(
        as_builder timeout "${PATH_PROBE_TIMEOUT_SECONDS}" "${builder_shell:-/bin/sh}" -lc \
            "printf '%s%s\n' '${PATH_MARKER}' \"\$PATH\"" </dev/null 2>/dev/null |
            sed -n "s/^.*${PATH_MARKER}//p" | tail -n 1 || true
    )"
else
    builder_home="${HOME}"
    user_path="${PATH}"
fi

# rustup's default location first, so a rustup toolchain wins over a system
# one, then the user's PATH, then the system PATH (root's under sudo), which
# still finds a distro-packaged cargo when the user's PATH could not be read.
cargo_bin=""
if [[ -x "${builder_home}/.cargo/bin/cargo" ]]; then
    cargo_bin="${builder_home}/.cargo/bin/cargo"
elif [[ -n "${user_path}" ]]; then
    cargo_bin="$(PATH="${user_path}" type -P cargo || true)"
fi
if [[ -z "${cargo_bin}" ]]; then
    cargo_bin="$(type -P cargo || true)"
fi
if [[ -z "${cargo_bin}" || ! -x "${cargo_bin}" ]]; then
    if [[ -z "${user_path}" ]]; then
        die "cargo was not found in ~/.cargo/bin or the system PATH, and your shell's PATH could not be read"
    fi
    die "cargo is not installed or not in PATH"
fi

echo "building ${BINARY_NAME} (release) as ${SUDO_USER:-${USER}}..."
as_builder "${cargo_bin}" build --release --locked

echo "installing to ${TARGET}..."
as_root install -Dm755 -- "target/release/${BINARY_NAME}" "${TARGET}"

echo "installed ${TARGET}"

# These checks describe the user's shell, so they use the user's PATH.
if [[ -z "${user_path}" ]]; then
    echo "note: could not read your shell's PATH; check with: command -v ${BINARY_NAME}" >&2
    exit 0
fi

resolved="$(PATH="${user_path}" type -P "${BINARY_NAME}" || true)"
if [[ -z "${resolved}" ]]; then
    echo "warning: ${BIN_DIR} is not in your PATH; run ${TARGET} directly or add it to PATH" >&2
elif [[ "${resolved}" != "${TARGET}" ]]; then
    echo "warning: '${BINARY_NAME}' resolves to ${resolved}, which comes before ${TARGET} in your PATH" >&2
fi

if [[ -z "$(PATH="${user_path}" type -P hyprctl || true)" ]]; then
    echo "warning: hyprctl is not in your PATH; autoclick needs it at runtime" >&2
fi
