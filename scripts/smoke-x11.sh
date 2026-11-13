#!/usr/bin/env bash
# Perch X11 smoke test: headless (Xvfb + openbox) or against a real session.
#
# Proves, by reading the X server's own state back:
#   - the pill and hover strip exist, are docks, and carry
#     ABOVE + STICKY + SKIP_TASKBAR + SKIP_PAGER, with no strut
#   - hovering the top edge (xdotool) expands the pill; leaving collapses it
#   - CLI verbs reach the running instance over the Unix socket
#   - a fullscreen window hides the pill, and un-fullscreening brings it back
#   - idle memory / CPU
# and saves screenshots as evidence.
#
# Usage: scripts/smoke-x11.sh [--out DIR] [--compositor none|picom] [--real-display]
#   --real-display  use the current $DISPLAY (e.g. a real Cinnamon session)
#                   instead of starting Xvfb + openbox.
set -euo pipefail

OUT=artifacts/linux
COMP=none
REAL=0
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$2; shift 2 ;;
        --compositor) COMP=$2; shift 2 ;;
        --real-display) REAL=1; shift ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
BIN=${PERCH_BIN:-target/release/perch}
[ -x "$BIN" ] || { echo "perch binary not found at $BIN" >&2; exit 2; }
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")

TMP=$(mktemp -d)
RESULTS="$OUT/x11-smoke-$COMP.txt"
: >"$RESULTS"
FAILS=0
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done
    # Wait for them, so a following run never races a dying X server or WM.
    for p in "${PIDS[@]:-}"; do wait "$p" 2>/dev/null || true; done
    rm -rf "$TMP"
}
trap cleanup EXIT

check() { # name ok detail
    if [ "$2" = 1 ]; then
        echo "[PASS] $1: $3" | tee -a "$RESULTS"
    else
        echo "[FAIL] $1: $3" | tee -a "$RESULTS"
        FAILS=$((FAILS + 1))
    fi
}
info() { echo "[INFO] $*" | tee -a "$RESULTS"; }

if [ "$REAL" = 0 ]; then
    # First free display number, so parallel or back-to-back runs never share one.
    N=97
    while [ -e "/tmp/.X$N-lock" ] || [ -e "/tmp/.X11-unix/X$N" ]; do N=$((N + 1)); done
    export DISPLAY=:$N
    Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 &
    PIDS+=($!)
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    openbox >/dev/null 2>&1 &
    PIDS+=($!)
    # A desktop always has its WM running before apps start; wait for it.
    for _ in $(seq 50); do
        xprop -root _NET_SUPPORTING_WM_CHECK 2>/dev/null | grep -q "window id" && break
        sleep 0.1
    done
    if [ "$COMP" = picom ]; then
        picom --backend xrender --no-vsync >/dev/null 2>&1 &
        PIDS+=($!)
        sleep 0.5
    fi
fi
info "display $DISPLAY, compositor=$COMP, $(xdpyinfo | grep dimensions | xargs)"

export HOME="$TMP/home"
export XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
export XDG_RUNTIME_DIR="$TMP/run"
mkdir -p "$HOME" "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
export PERCH_STATUS_FILE="$TMP/status.json"
export PERCH_LOG=stderr
STATUS() { jq -r "$1" "$PERCH_STATUS_FILE" 2>/dev/null || echo ""; }

# Turn on the features the end-to-end checks below exercise.
cat >"$TMP/config.toml" <<CFG
[features]
media = true
todo = true
clipboard = true
shelf = true
CFG

T0=$(date +%s%N)
"$BIN" --config "$TMP/config.toml" >"$TMP/perch.log" 2>&1 &
PID=$!
PIDS+=("$PID")
for _ in $(seq 100); do
    [ "$(STATUS .frames)" != "" ] && [ "$(STATUS .frames)" -gt 0 ] 2>/dev/null && break
    sleep 0.1
done
T1=$(date +%s%N)
check process_alive "$(kill -0 "$PID" 2>/dev/null && echo 1 || echo 0)" "pid $PID"
info "first frame (from the app's own clock): $(STATUS .first_frame_ms) ms; wall time to first status: $(( (T1 - T0) / 1000000 )) ms"

PILL=$(xdotool search --name '^Perch$' | head -1 || true)
HOT=$(xdotool search --name '^Perch hot zone$' | head -1 || true)
check pill_window "$([ -n "$PILL" ] && echo 1 || echo 0)" "id ${PILL:-none}"
check hot_window "$([ -n "$HOT" ] && echo 1 || echo 0)" "id ${HOT:-none}"
if [ -z "$PILL" ] || [ -z "$HOT" ]; then
    cat "$TMP/perch.log"
    exit 1
fi

TYPE=$(xprop -id "$PILL" _NET_WM_WINDOW_TYPE)
# The WM applies the requested states once it has managed the window.
for _ in $(seq 50); do
    xprop -id "$PILL" _NET_WM_STATE | grep -q ABOVE && break
    sleep 0.1
done
STATE=$(xprop -id "$PILL" _NET_WM_STATE)
STRUT=$(xprop -id "$PILL" _NET_WM_STRUT_PARTIAL _NET_WM_STRUT)
check dock_type "$(echo "$TYPE" | grep -q DOCK && echo 1 || echo 0)" "$TYPE"
DESK=$(xprop -id "$PILL" _NET_WM_DESKTOP)
for s in ABOVE STICKY SKIP_TASKBAR SKIP_PAGER; do
    if [ "$s" = STICKY ] && echo "$DESK" | grep -q 4294967295; then
        check state_STICKY 1 "on all desktops ($DESK)"
        continue
    fi
    check "state_$s" "$(echo "$STATE" | grep -q "_NET_WM_STATE_$s" && echo 1 || echo 0)" "$s"
done
check no_strut "$(echo "$STRUT" | grep -q 'not found' && echo 1 || echo 0)" "never reserves screen space"
HOT_H=$(xwininfo -id "$HOT" | awk '/Height:/ {print $2}')
check hot_zone_thin "$([ "$HOT_H" -le 8 ] && echo 1 || echo 0)" "${HOT_H}px"

geom() { xwininfo -id "$PILL" | awk '/Absolute upper-left X/ {x=$4} /Absolute upper-left Y/ {y=$4} /Width:/ {w=$2} /Height:/ {h=$2} END {print x, y, w, h}'; }
xdotool mousemove 40 900
sleep 1
read -r X Y W H <<<"$(geom)"
info "collapsed: ${W}x${H} at $X,$Y"
check collapsed_top_center "$([ "$Y" -eq 0 ] && [ $((X + W / 2)) -ge 950 ] && [ $((X + W / 2)) -le 970 ] && echo 1 || echo 0)" "centre x $((X + W / 2)), y $Y"

xdotool mousemove 960 0
sleep 0.8
read -r X2 Y2 W2 H2 <<<"$(geom)"
info "hovered: ${W2}x${H2}; state=$(STATUS .state)"
check hover_expands "$([ "$H2" -gt "$H" ] && [ "$W2" -gt "$W" ] && echo 1 || echo 0)" "${W}x${H} -> ${W2}x${H2}"
import -window root -crop 560x220+680+0 "$OUT/x11-expanded-$COMP.png" 2>/dev/null || true

xdotool mousemove 960 800
sleep 1
read -r _ _ W3 H3 <<<"$(geom)"
check leave_collapses "$([ "$H3" -eq "$H" ] && echo 1 || echo 0)" "${W2}x${H2} -> ${W3}x${H3}"
import -window root -crop 560x80+680+0 "$OUT/x11-collapsed-$COMP.png" 2>/dev/null || true

# CLI verbs over the Unix socket.
"$BIN" toggle-notch
sleep 0.8
check cli_toggle_pins_open "$([ "$(STATUS .state)" = Expanded ] && echo 1 || echo 0)" "state $(STATUS .state)"
"$BIN" toggle-notch
sleep 0.8
check cli_toggle_closes "$([ "$(STATUS .state)" = Collapsed ] && echo 1 || echo 0)" "state $(STATUS .state)"
"$BIN" todo "smoke test item"
sleep 0.5
check cli_todo "$([ "$(STATUS .todos)" = 1 ] && echo 1 || echo 0)" "todos=$(STATUS .todos)"

# Now playing over MPRIS, end to end (mock player -> D-Bus -> Perch), driven
# with playerctl like a real desktop would. Needs a session bus.
MOCK=${PERCH_MOCK_PLAYER:-}
if [ -n "$MOCK" ] && [ -x "$MOCK" ] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    "$MOCK" "Smoke Song" "Smoke Band" &
    PIDS+=($!)
    for _ in $(seq 50); do [ "$(STATUS .track)" = "Smoke Band - Smoke Song" ] && break; sleep 0.1; done
    check mpris_track "$([ "$(STATUS .track)" = "Smoke Band - Smoke Song" ] && echo 1 || echo 0)" "track='$(STATUS .track)'"
    playerctl --player=perchmock play-pause || info "playerctl failed"
    for _ in $(seq 30); do [ "$(STATUS .playing)" = false ] && break; sleep 0.1; done
    check mpris_playerctl_pause "$([ "$(STATUS .playing)" = false ] && echo 1 || echo 0)" "playing=$(STATUS .playing)"
    "$BIN" play-pause
    for _ in $(seq 30); do [ "$(STATUS .playing)" = true ] && break; sleep 0.1; done
    check mpris_cli_resume "$([ "$(STATUS .playing)" = true ] && echo 1 || echo 0)" "playing=$(STATUS .playing)"
    xdotool mousemove 960 0
    sleep 0.8
    import -window root -crop 560x220+680+0 "$OUT/x11-media-$COMP.png" 2>/dev/null || true
    xdotool mousemove 40 900
    sleep 0.8
else
    info "MPRIS end-to-end skipped (set PERCH_MOCK_PLAYER and run under dbus-run-session)"
fi

# Clipboard board: normal copies are recorded, password-manager copies are
# not (x-kde-passwordManagerHint, as set by KeePassXC).
CLIP_OWNER=${PERCH_CLIP_OWNER:-}
if [ -n "$CLIP_OWNER" ] && [ -x "$CLIP_OWNER" ]; then
    printf 'hello from the smoke test' | xclip -selection clipboard
    for _ in $(seq 50); do [ "$(STATUS .clipboard)" = 1 ] && break; sleep 0.1; done
    check clipboard_records_copy "$([ "$(STATUS .clipboard)" = 1 ] && echo 1 || echo 0)" "entries=$(STATUS .clipboard)"
    "$CLIP_OWNER" "hunter2" --secret &
    CO=$!
    PIDS+=("$CO")
    sleep 1.5
    check clipboard_skips_password_manager "$([ "$(STATUS .clipboard)" = 1 ] && echo 1 || echo 0)" "entries=$(STATUS .clipboard)"
    printf 'second copy' | xclip -selection clipboard
    for _ in $(seq 50); do [ "$(STATUS .clipboard)" = 2 ] && break; sleep 0.1; done
    check clipboard_records_after_secret "$([ "$(STATUS .clipboard)" = 2 ] && echo 1 || echo 0)" "entries=$(STATUS .clipboard)"
    "$BIN" tab clips
    xdotool mousemove 960 0
    sleep 0.8
    import -window root -crop 560x220+680+0 "$OUT/x11-clipboard-$COMP.png" 2>/dev/null || true
    xdotool mousemove 40 900
    sleep 0.8
else
    info "clipboard end-to-end skipped (set PERCH_CLIP_OWNER)"
fi

# Fullscreen auto-hide (EWMH _NET_WM_STATE_FULLSCREEN via the WM). The
# victim is Perch's own settings window: a normal, focusable top-level.
"$BIN" settings
sleep 1
wmctrl -a "Perch settings" || true
sleep 0.5
wmctrl -r "Perch settings" -b add,fullscreen
sleep 1
check fullscreen_hides "$([ "$(STATUS .visible)" = false ] && echo 1 || echo 0)" "visible=$(STATUS .visible)"
wmctrl -r "Perch settings" -b remove,fullscreen
sleep 1
check unfullscreen_shows "$([ "$(STATUS .visible)" = true ] && echo 1 || echo 0)" "visible=$(STATUS .visible)"
wmctrl -c "Perch settings" || true

# A window-manager restart ("Restart Cinnamon") must not lose keep-above.
if [ "$REAL" = 0 ]; then
    OLD_WM=$(xprop -root _NET_SUPPORTING_WM_CHECK)
    openbox --replace >/dev/null 2>&1 &
    PIDS+=($!)
    for _ in $(seq 50); do
        [ "$(xprop -root _NET_SUPPORTING_WM_CHECK)" != "$OLD_WM" ] && break
        sleep 0.1
    done
    for _ in $(seq 50); do
        xprop -id "$PILL" _NET_WM_STATE | grep -q ABOVE && break
        sleep 0.1
    done
    check wm_restart_keeps_above "$(xprop -id "$PILL" _NET_WM_STATE | grep -q ABOVE && echo 1 || echo 0)" "$(xprop -id "$PILL" _NET_WM_STATE)"
fi

# Resource usage after settling.
xdotool mousemove 40 900
sleep 2
RSS_KB=$(awk '/VmRSS/ {print $2}' /proc/"$PID"/status)
T_A=$(awk '{print $14 + $15}' /proc/"$PID"/stat)
sleep 10
T_B=$(awk '{print $14 + $15}' /proc/"$PID"/stat)
TICKS=$(getconf CLK_TCK)
info "VmRSS $((RSS_KB / 1024)) MB; CPU over 10 s idle: $(( (T_B - T_A) * 1000 / TICKS )) ms"
check idle_rss_under_40mb "$([ "$RSS_KB" -lt 40960 ] && echo 1 || echo 0)" "$((RSS_KB / 1024)) MB"
check idle_cpu_near_zero "$([ $((T_B - T_A)) -le 2 ] && echo 1 || echo 0)" "$((T_B - T_A)) ticks in 10 s"

"$BIN" quit
for _ in $(seq 30); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
check quits_cleanly "$(kill -0 "$PID" 2>/dev/null && echo 0 || echo 1)" "perch quit"

echo "--- perch log ---" >>"$RESULTS"
cat "$TMP/perch.log" >>"$RESULTS"
if [ "$FAILS" -gt 0 ]; then
    echo "X11 smoke test FAILED ($FAILS)"; exit 1
fi
echo "X11 smoke test passed ($COMP)"
