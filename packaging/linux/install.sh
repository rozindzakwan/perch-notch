#!/usr/bin/env bash
# Install Perch for the current user without a package manager
# (~/.local/bin, ~/.local/share/applications, hicolor icons).
#
# Usage:
#   ./install.sh [path/to/perch]   install (default: ./perch, then target/release/perch)
#   ./install.sh --uninstall       remove everything this script installed
#
# On Linux Mint / Ubuntu prefer the .deb (sudo apt install ./perch_*.deb).
set -euo pipefail

BIN_DIR="${XDG_BIN_HOME:-$HOME/.local/bin}"
DATA_DIR="${XDG_DATA_HOME:-$HOME/.local/share}"
APP_DIR="$DATA_DIR/applications"
ICON_DIR="$DATA_DIR/icons/hicolor"
HERE="$(cd "$(dirname "$0")" && pwd)"
ROOT="$(cd "$HERE/../.." 2>/dev/null && pwd || echo "$HERE")"

if [ "${1:-}" = "--uninstall" ]; then
    pkill -x perch 2>/dev/null || true
    rm -f "$BIN_DIR/perch" "$APP_DIR/perch.desktop" \
        "$ICON_DIR/256x256/apps/perch.png" "$ICON_DIR/128x128/apps/perch.png" \
        "${XDG_CONFIG_HOME:-$HOME/.config}/autostart/perch.desktop"
    echo "Perch removed. Settings remain in ~/.config/perch (delete it for a clean slate)."
    exit 0
fi

SRC="${1:-}"
for c in "$SRC" "$HERE/perch" "$ROOT/target/release/perch"; do
    if [ -n "$c" ] && [ -x "$c" ]; then SRC="$c"; break; fi
done
[ -x "${SRC:-}" ] || { echo "perch binary not found; build it with: cargo build --release" >&2; exit 1; }

find_asset() { for d in "$HERE" "$ROOT/assets" "$ROOT/packaging/linux"; do [ -f "$d/$1" ] && { echo "$d/$1"; return; }; done; }

install -Dm755 "$SRC" "$BIN_DIR/perch"
DESKTOP="$(find_asset perch.desktop)"
[ -n "$DESKTOP" ] && sed "s|^Exec=perch|Exec=$BIN_DIR/perch|" "$DESKTOP" | install -Dm644 /dev/stdin "$APP_DIR/perch.desktop"
ICON256="$(find_asset perch.png)"
ICON128="$(find_asset perch-128.png)"
[ -n "$ICON256" ] && install -Dm644 "$ICON256" "$ICON_DIR/256x256/apps/perch.png"
[ -n "$ICON128" ] && install -Dm644 "$ICON128" "$ICON_DIR/128x128/apps/perch.png"
command -v update-desktop-database >/dev/null && update-desktop-database "$APP_DIR" 2>/dev/null || true
command -v gtk-update-icon-cache >/dev/null && gtk-update-icon-cache -q "$ICON_DIR" 2>/dev/null || true

echo "Perch installed to $BIN_DIR/perch"
case ":$PATH:" in *":$BIN_DIR:"*) ;; *) echo "Note: $BIN_DIR is not on your PATH." ;; esac
echo "Optional: sudo apt install tesseract-ocr (screenshot text recognition)"
echo "Start it from the menu, or run: perch"
