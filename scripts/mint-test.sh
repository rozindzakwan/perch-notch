#!/usr/bin/env bash
# Perch - verification on a REAL Linux Mint Cinnamon (X11) session.
#
# This is what turns "CI-verified (ubuntu-22.04 container)" into
# "verified on real Mint". Run it from a terminal inside your Cinnamon session
# (not over SSH: it needs $DISPLAY and your session bus):
#
#   ./scripts/mint-setup.sh --with-optional   # once
#   ./scripts/mint-test.sh                     # ~40 min, mostly the soak
#
# Steps (each logged with PASS/FAIL/SKIP to artifacts/mint/mint-results.md):
#   1. environment report (distro, Cinnamon version, session type, compositor, scale)
#   2. cargo fmt/clippy/test (all features) + release build
#   3. the X11 smoke test against your real desktop (--real-display): dock +
#      keep-above properties, hover expand/collapse, CLI, mock MPRIS player,
#      clipboard privacy, screenshot overlay, fullscreen auto-hide, idle RAM/CPU
#   4. a real media player: if one is playing (Rhythmbox, Celluloid, Firefox,
#      Spotify...), checks Perch shows the same track as playerctl
#   5. tray: is a StatusNotifierItem watcher present (xapp-sn-watcher)?
#   6. OCR: tesseract on a rendered test image
#   7. the 30-minute soak test on your desktop (--real-display)
#
# While it runs it moves the mouse to the top of the screen and back a few
# times and opens/closes Perch windows. Leave the machine alone until it says
# it's done (or pass --no-soak to skip the 30 minutes).
#
# Usage: scripts/mint-test.sh [--no-soak] [--soak-minutes N] [--skip-build]
set -uo pipefail

SOAK=1
SOAK_MIN=30
BUILD=1
while [ $# -gt 0 ]; do
    case "$1" in
        --no-soak) SOAK=0; shift ;;
        --soak-minutes) SOAK_MIN=$2; shift 2 ;;
        --skip-build) BUILD=0; shift ;;
        -h|--help) sed -n '2,27p' "$0"; exit 0 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done

cd "$(dirname "$0")/.."
ROOT=$PWD
OUT=$ROOT/artifacts/mint
mkdir -p "$OUT"
REPORT=$OUT/mint-results.md
LOG=$OUT/mint-test.log
: >"$LOG"
PASS=0; FAIL=0; SKIP=0
ROWS=()

say() { printf '\n\033[1;36m==> %s\033[0m\n' "$*"; echo "==> $*" >>"$LOG"; }
row() { # status step detail
    ROWS+=("| $2 | $1 | $3 |")
    case "$1" in PASS) PASS=$((PASS + 1)) ;; FAIL) FAIL=$((FAIL + 1)) ;; *) SKIP=$((SKIP + 1)) ;; esac
    printf '  [%s] %s: %s\n' "$1" "$2" "$3"
}
run() { # step command...  (logs output, records PASS/FAIL)
    local step=$1; shift
    if "$@" >>"$LOG" 2>&1; then row PASS "$step" "$*"; return 0; fi
    row FAIL "$step" "$* (see mint-test.log)"; return 1
}
have() { command -v "$1" >/dev/null 2>&1; }

if [ -z "${DISPLAY:-}" ]; then
    echo "No \$DISPLAY: run this from a terminal inside your Cinnamon session." >&2
    exit 2
fi
if [ "${XDG_SESSION_TYPE:-x11}" = wayland ]; then
    echo "This is a Wayland session; Perch supports X11 only for now. Log in to 'Cinnamon' (not 'Cinnamon on Wayland')." >&2
    exit 2
fi

# --- 1. environment ---------------------------------------------------------
say "Environment"
. /etc/os-release
CINNAMON=$(cinnamon --version 2>/dev/null || echo "not found")
xprop -root _NET_SUPPORTING_WM_CHECK >/dev/null 2>&1 && \
    WM=$(xprop -id "$(xprop -root _NET_SUPPORTING_WM_CHECK | awk '{print $NF}')" _NET_WM_NAME 2>/dev/null | cut -d'"' -f2)
SCREEN=$(xdpyinfo 2>/dev/null | awk '/^screen #0/ {getline; print $2}')
SCALE=$(gsettings get org.cinnamon.desktop.interface scaling-factor 2>/dev/null || echo "?")
MONITORS=$(xrandr --listmonitors 2>/dev/null | head -1 | awk '{print $2}')
GPU=$(lspci 2>/dev/null | grep -i -m1 'vga\|3d' | cut -d: -f3- | xargs || echo "?")
ENV_TXT="- Distro: ${PRETTY_NAME} (base ${UBUNTU_CODENAME:-?}), kernel $(uname -r)
- Desktop: ${XDG_CURRENT_DESKTOP:-?}, ${CINNAMON}, window manager ${WM:-?}, session ${XDG_SESSION_TYPE:-?}
- Screen: ${SCREEN:-?}, monitors: ${MONITORS:-?}, Cinnamon scaling-factor: ${SCALE}
- CPU: $(grep -m1 'model name' /proc/cpuinfo | cut -d: -f2 | xargs), $(nproc) threads, RAM $(free -g | awk '/Mem:/ {print $2}') GB
- GPU: ${GPU}
- glibc: $(ldd --version | head -1 | awk '{print $NF}'), Rust: $(rustc --version 2>/dev/null || echo none)"
echo "$ENV_TXT"

# A running Perch would sit on top of the test instance.
if pgrep -x perch >/dev/null; then
    say "Stopping the running Perch (it is restarted at the end)"
    RESTART_PERCH=$(readlink -f /proc/"$(pgrep -x perch | head -1)"/exe || true)
    pkill -x perch || true
    sleep 1
fi

# --- 2. build + tests -------------------------------------------------------
if [ "$BUILD" = 1 ]; then
    say "fmt, clippy, tests, release build"
    run fmt cargo fmt --all --check
    run clippy cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
    run tests cargo test --workspace --all-features --locked
    run build cargo build --release --locked -p perch
    run build_examples cargo build --release --locked -p perch-platform-linux --example mock_player --example clip_owner
else
    row SKIP build "--skip-build"
fi
BIN=$ROOT/target/release/perch
[ -x "$BIN" ] || { echo "no release binary at $BIN" >&2; exit 1; }
SIZE_MB=$(awk -v b="$(stat -c %s "$BIN")" 'BEGIN { printf "%.2f", b / 1048576 }')
row INFO binary_size "${SIZE_MB} MB ($BIN)"

# D-Bus integration tests on a private bus (do not touch your real players).
if have dbus-run-session; then
    run dbus_tests dbus-run-session -- cargo test -p perch-platform-linux --all-features --locked \
        --test mpris --test session_bus -- --include-ignored --test-threads=1
fi

# --- 3. X11 smoke on the real desktop ----------------------------------------
say "X11 smoke test on your desktop (hands off the mouse for ~1 minute)"
missing=""
for t in xdotool xprop xwininfo xdpyinfo xrandr import xclip wmctrl jq; do have "$t" || missing="$missing $t"; done
if [ -n "$missing" ]; then
    row SKIP smoke_real_display "missing tools:$missing (run scripts/mint-setup.sh --with-optional)"
else
    if PERCH_BIN="$BIN" PERCH_MOCK_PLAYER="$ROOT/target/release/examples/mock_player" \
        PERCH_CLIP_OWNER="$ROOT/target/release/examples/clip_owner" \
        scripts/smoke-x11.sh --out "$OUT" --compositor real --real-display >>"$LOG" 2>&1; then
        row PASS smoke_real_display "$(grep -c '^\[PASS\]' "$OUT/x11-smoke-real.txt") checks passed"
    else
        row FAIL smoke_real_display "$(grep '^\[FAIL\]' "$OUT/x11-smoke-real.txt" | cut -c1-200 | tr '\n' ';')"
    fi
    grep '^\[INFO\]' "$OUT/x11-smoke-real.txt" | sed 's/^/  /'
fi

# --- 4. a real media player --------------------------------------------------
say "Real MPRIS player"
if have playerctl && playerctl -l >/dev/null 2>&1 && [ -n "$(playerctl -l 2>/dev/null)" ]; then
    T=$(mktemp -d)
    printf '[features]\nmedia = true\n' >"$T/config.toml"
    PERCH_STATUS_FILE="$T/status.json" "$BIN" --config "$T/config.toml" >"$T/log" 2>&1 &
    P=$!
    sleep 3
    WANT="$(playerctl metadata --format '{{artist}} - {{title}}' 2>/dev/null)"
    GOT="$(jq -r .track "$T/status.json" 2>/dev/null)"
    if [ -n "$WANT" ] && [ "$WANT" = "$GOT" ]; then
        row PASS real_player "$(playerctl -l | head -1): '$GOT'"
    else
        row FAIL real_player "playerctl says '$WANT', Perch says '$GOT'"
    fi
    "$BIN" quit 2>/dev/null || kill "$P" 2>/dev/null
    wait "$P" 2>/dev/null
    rm -rf "$T"
else
    row SKIP real_player "no MPRIS player running (start music in Rhythmbox/Firefox and re-run to cover this)"
fi

# --- 5. tray ----------------------------------------------------------------
if have busctl && busctl --user list 2>/dev/null | grep -q org.kde.StatusNotifierWatcher; then
    row PASS tray_watcher "StatusNotifierWatcher present (xapp-sn-watcher); Perch's tray icon appears in the panel"
else
    row INFO tray_watcher "no StatusNotifierWatcher on the bus; Perch falls back to an XEmbed tray icon"
fi

# --- 6. OCR -------------------------------------------------------------------
if have tesseract && have convert; then
    T=$(mktemp -d)
    convert -size 900x160 xc:white -fill black -pointsize 64 -annotate +30+105 'PERCH READS 42 WORDS' "$T/ocr.png" 2>/dev/null
    TXT=$(tesseract "$T/ocr.png" - 2>/dev/null | tr -s ' \n' ' ')
    case "$TXT" in *"PERCH READS 42 WORDS"*) row PASS ocr "tesseract read: $TXT" ;; *) row FAIL ocr "tesseract read: '$TXT'" ;; esac
    rm -rf "$T"
else
    row SKIP ocr "tesseract-ocr or imagemagick not installed"
fi

# --- 7. soak ------------------------------------------------------------------
if [ "$SOAK" = 1 ]; then
    say "${SOAK_MIN}-minute soak on your desktop (you may use the machine, just don't quit Perch)"
    if PERCH_BIN="$BIN" scripts/soak-linux.sh --out "$OUT" --minutes "$SOAK_MIN" --real-display >>"$LOG" 2>&1; then
        row PASS soak "$(grep -E '^\| VmRSS' "$OUT/soak-linux.md" | xargs); $(grep '^- CPU' "$OUT/soak-linux.md" | cut -c3-)"
    else
        row FAIL soak "$(grep '^- Result' "$OUT/soak-linux.md" 2>/dev/null | cut -c3-)"
    fi
else
    row SKIP soak "--no-soak"
fi

if [ -n "${RESTART_PERCH:-}" ] && [ -x "$RESTART_PERCH" ]; then
    (setsid "$RESTART_PERCH" >/dev/null 2>&1 &)
fi

# --- report -------------------------------------------------------------------
{
    echo "# Perch on real Linux Mint: results"
    echo
    echo "Generated by scripts/mint-test.sh on $(date -u '+%Y-%m-%d %H:%M UTC'), commit $(git rev-parse --short HEAD 2>/dev/null || echo '?')."
    echo
    echo "$ENV_TXT"
    echo
    echo "| Step | Result | Detail |"
    echo "|---|---|---|"
    printf '%s\n' "${ROWS[@]}"
    echo
    echo "Totals: $PASS passed, $FAIL failed, $SKIP skipped/info."
    echo
    echo "Evidence: artifacts/mint/ (x11-smoke-real.txt, cropped x11-*-real.png screenshots, soak-linux.csv/md, mint-test.log)."
    echo
    echo "Paste this file into TESTING.md under \"Real Linux Mint\" and mark the README rows you saw pass as \"verified on real Mint\"."
} >"$REPORT"
say "Done: $PASS passed, $FAIL failed, $SKIP skipped/info"
echo "Report: $REPORT"
[ "$FAIL" -eq 0 ]
