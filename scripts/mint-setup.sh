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

# Required build and runtime libraries.
#   build-essential/pkg-config : cc + linker for the Rust build
#   libx11/libxcb/libxkbcommon : winit's X11 backend
#   libxcb-randr/xfixes        : monitor enumeration and selection events
#   libwayland/libxkbcommon-x11: winit's Wayland backend (best effort path)
#   libdbus-1-dev              : MPRIS over D-Bus (zbus), notifications
#   libasound2-dev             : cpal/rodio (white noise)
#   libgtk-3-dev + libayatana  : tray icon (StatusNotifierItem via tray-icon)
#   libssl-dev                 : TLS for the opt-in LRCLIB lyrics fetch
PKGS=(
    build-essential
    pkg-config
    curl
    git
    libx11-dev
    libxcb1-dev
    libxcb-randr0-dev
    libxcb-xfixes0-dev
    libxcb-shape0-dev
    libxcb-cursor-dev
    libxkbcommon-dev
    libxkbcommon-x11-dev
    libwayland-dev
    libdbus-1-dev
    libasound2-dev
    libssl-dev
    libgtk-3-dev
    libayatana-appindicator3-dev
)

# Optional runtime helpers, declared as Recommends in the .deb.
OPTIONAL_PKGS=(
    xdotool            # paste-after-dictation via XTEST
    tesseract-ocr      # screenshot OCR
    playerctl          # used by mint-test.sh to drive a real MPRIS player
    scrot              # screenshot evidence in mint-test.sh
    x11-utils          # xprop/xwininfo, used by mint-test.sh assertions
    sysstat            # pidstat, used by the soak test
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
for t in xdotool playerctl scrot xprop pidstat; do
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
  ./scripts/mint-test.sh          # builds, tests, and drives the real session

If you want the Debian package:

  cargo install cargo-deb
  cargo deb -p perch

Notes:
  * Perch needs a compositor for a transparent pill. Cinnamon provides one
    (Muffin). Without compositing, the pill falls back to an opaque background.
  * On a Wayland session Perch starts but the dock/above window properties are
    X11-only; see the README's support table.
EOF
