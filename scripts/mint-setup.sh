#!/usr/bin/env bash
# Perch - development/runtime setup for Linux Mint (Cinnamon) and other
# Ubuntu-based distributions.
#
# Installs the apt packages Perch needs to build and run, plus rustup if no
# Rust toolchain is present. Safe to re-run.
#
# Usage:  ./scripts/mint-setup.sh [--no-rustup] [--with-optional]
set -euo pipefail

WITH_RUSTUP=1
WITH_OPTIONAL=0
for arg in "$@"; do
    case "$arg" in
        --no-rustup) WITH_RUSTUP=0 ;;
        --with-optional) WITH_OPTIONAL=1 ;;
        -h|--help)
            sed -n '2,10p' "$0"
            exit 0
            ;;
        *) echo "unknown argument: $arg" >&2; exit 2 ;;
    esac
done

say() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; }
warn() { printf '\033[1;33mwarning:\033[0m %s\n' "$*" >&2; }

if ! command -v apt-get >/dev/null 2>&1; then
    echo "This script targets Ubuntu-based distributions (Linux Mint). No apt-get found." >&2
    exit 1
fi

say "Detected system"
if [ -r /etc/os-release ]; then
    # shellcheck disable=SC1091
    . /etc/os-release
    echo "  distro : ${PRETTY_NAME:-unknown}"
    echo "  base   : ${UBUNTU_CODENAME:-${VERSION_CODENAME:-unknown}}"
fi
echo "  session: ${XDG_SESSION_TYPE:-unset} (DISPLAY=${DISPLAY:-unset} WAYLAND_DISPLAY=${WAYLAND_DISPLAY:-unset})"
echo "  desktop: ${XDG_CURRENT_DESKTOP:-unknown}"

# Required build libraries (the same set docker/linux-ci.Dockerfile uses).
#   build-essential/pkg-config : cc + linker for the Rust build
#   libx11/libxcb/libxkbcommon : winit X11 backend, x11rb (dock, XShape, XFixes)
#   libwayland-dev             : winit Wayland backend (compiled in, not supported)
#   libpulse-dev               : per-app volume mixer (libpulse; works on PipeWire)
#   libasound2-dev             : audio output (white noise) and dictation input
#   libv4l-dev                 : camera mirror (v4l2)
#   cmake/clang/libclang-dev   : only for the optional dictation build (whisper.cpp)
# No GTK: the tray is a pure-Rust StatusNotifierItem (ksni), D-Bus is pure Rust (zbus).
PKGS=(
    build-essential
    pkg-config
    curl
    git
    libx11-dev
    libxcb1-dev
    libxkbcommon-dev
    libxkbcommon-x11-dev
    libwayland-dev
    libpulse-dev
    libasound2-dev
    libv4l-dev
    cmake
    clang
    libclang-dev
)

# Optional runtime helpers and the tools scripts/mint-test.sh drives.
OPTIONAL_PKGS=(
    tesseract-ocr      # screenshot OCR (the .deb Recommends it)
    playerctl          # mint-test.sh: drive a real MPRIS player
    xdotool            # mint-test.sh: synthetic hover / clicks
    x11-utils          # xprop, xwininfo, xdpyinfo
    x11-xserver-utils  # xrandr (monitor geometry)
    imagemagick        # import: cropped screenshots as evidence
    xclip              # clipboard checks
    wmctrl             # fullscreen auto-hide check
    jq                 # reads Perch status JSON
    sysstat            # pidstat
)

say "Updating package lists"
sudo apt-get update -qq

say "Installing required packages"
missing=()
for p in "${PKGS[@]}"; do
    if ! dpkg -s "$p" >/dev/null 2>&1; then missing+=("$p"); fi
done
if [ ${#missing[@]} -eq 0 ]; then
    echo "  all required packages already installed"
else
    echo "  installing: ${missing[*]}"
    sudo apt-get install -y "${missing[@]}"
fi

if [ "$WITH_OPTIONAL" -eq 1 ]; then
    say "Installing optional packages"
    sudo apt-get install -y "${OPTIONAL_PKGS[@]}" || warn "some optional packages are unavailable; continuing"
else
    echo
    echo "Optional helpers were skipped. For the full feature set and for"
    echo "scripts/mint-test.sh, re-run with --with-optional, or install:"
    echo "  sudo apt-get install ${OPTIONAL_PKGS[*]}"
fi

if [ "$WITH_RUSTUP" -eq 1 ]; then
    if command -v cargo >/dev/null 2>&1; then
        say "Rust already present"
        cargo --version
        rustc --version
    else
        say "Installing rustup (stable, minimal profile plus clippy and rustfmt)"
        curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs \
            | sh -s -- -y --default-toolchain stable --profile minimal --component clippy rustfmt
        # shellcheck disable=SC1091
        . "$HOME/.cargo/env"
        cargo --version
    fi
fi

say "Checking the tools mint-test.sh needs"
for t in xdotool playerctl xprop xrandr import xclip wmctrl jq tesseract; do
    if command -v "$t" >/dev/null 2>&1; then
        printf '  %-10s ok\n' "$t"
    else
        printf '  %-10s MISSING (mint-test.sh will skip the steps that need it)\n' "$t"
    fi
done

say "Done"
cat <<'EOF'
Next steps on this machine:

  cargo build --release
  ./scripts/mint-test.sh          # builds, tests, drives the real session (~40 min)

If you want the Debian package:

  cargo install cargo-deb
  cargo deb -p perch

Notes:
  * Cinnamon composites (Muffin), so the pill is translucent. Without a
    compositor it is opaque with XShape-rounded corners.
  * Wayland sessions are not supported yet; Perch needs an X11 session
    (Cinnamon on Mint is X11 by default). See the README.
EOF
