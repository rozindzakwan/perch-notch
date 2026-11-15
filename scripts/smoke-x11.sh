#!/usr/bin/env bash
# Perch X11 smoke test: headless (Xvfb + openbox) or against a real session.
#
# Proves, by reading the X server's own state back:
#   - the pill and hover strip exist, are docks, and carry
#     ABOVE + STICKY + SKIP_TASKBAR + SKIP_PAGER, with no strut
#   - hovering the top edge (xdotool) expands the pill; leaving collapses it
#   - CLI verbs reach the running instance over the Unix socket
#   - over a fullscreen or maximized window the notch is a thin stripe that
#     opens only after a 3 s hover
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
# Geometry of the monitor the pill sits on (the primary one), so the same
# checks work on Xvfb's 1920x1080 and on a real desktop of any size.
PRIMARY=$(xrandr --query 2>/dev/null | awk '/ connected primary/ {for (i=1;i<=NF;i++) if ($i ~ /^[0-9]+x[0-9]+\+/) {print $i; exit}}')
[ -n "$PRIMARY" ] || PRIMARY="$(xdpyinfo | awk '/dimensions/ {print $2}')+0+0"
IFS='x+' read -r MW MH MX MY <<<"$PRIMARY"
CX=$((MX + MW / 2))       # horizontal centre: where the pill is
TOP=$MY                   # the monitor's top edge: the hover strip
AWAY=$((MY + MH * 5 / 6)) # somewhere well below the pill
CROPX=$((CX - 280))
info "primary monitor ${MW}x${MH}+${MX}+${MY}"

export HOME="$TMP/home"
export XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
if [ -z "${DBUS_SESSION_BUS_ADDRESS:-}" ] && [ -S "${XDG_RUNTIME_DIR:-/nonexistent}/bus" ]; then
    export DBUS_SESSION_BUS_ADDRESS="unix:path=$XDG_RUNTIME_DIR/bus"
fi
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
screen_time = true
screenshot = true
pomodoro = true
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
xdotool mousemove $((MX + 40)) $AWAY
sleep 1
read -r X Y W H <<<"$(geom)"
info "collapsed: ${W}x${H} at $X,$Y"
check collapsed_top_center "$([ "$Y" -eq 0 ] && [ $((X + W / 2)) -ge 950 ] && [ $((X + W / 2)) -le 970 ] && echo 1 || echo 0)" "centre x $((X + W / 2)), y $Y"

xdotool mousemove $CX $TOP
sleep 0.8
read -r X2 Y2 W2 H2 <<<"$(geom)"
info "hovered: ${W2}x${H2}; state=$(STATUS .state)"
check hover_expands "$([ "$H2" -gt "$H" ] && [ "$W2" -gt "$W" ] && echo 1 || echo 0)" "${W}x${H} -> ${W2}x${H2}"
import -window root -crop 560x340+$CROPX+$TOP "$OUT/x11-expanded-$COMP.png" 2>/dev/null || true

xdotool mousemove $CX $((MY + MH * 3 / 4))
sleep 1
read -r _ _ W3 H3 <<<"$(geom)"
check leave_collapses "$([ "$H3" -eq "$H" ] && echo 1 || echo 0)" "${W2}x${H2} -> ${W3}x${H3}"
import -window root -crop 560x80+$CROPX+$TOP "$OUT/x11-collapsed-$COMP.png" 2>/dev/null || true

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

# A to-do falling due shows as a banner in the notch, then goes away.
"$BIN" todo +2s Smoke banner
SHOWN=""
for _ in $(seq 60); do SHOWN=$(STATUS .banner); [ -n "$SHOWN" ] && break; sleep 0.1; done
check todo_due_banners "$([ -n "$SHOWN" ] && echo 1 || echo 0)" "banner='$SHOWN'"
sleep 0.4
import -window root -crop 560x100+$CROPX+$TOP "$OUT/x11-banner-$COMP.png" 2>/dev/null || true
GONE=0
for _ in $(seq 80); do [ -z "$(STATUS .banner)" ] && { GONE=1; break; }; sleep 0.1; done
check banner_goes_away "$GONE" "$([ "$GONE" = 1 ] && echo 'cleared within 8 s' || echo "still '$(STATUS .banner)'")"

# Now playing over MPRIS, end to end (mock player -> D-Bus -> Perch), driven
# with playerctl like a real desktop would. Needs a session bus.
MOCK=${PERCH_MOCK_PLAYER:-}
if [ -n "$MOCK" ] && [ -x "$MOCK" ] && [ -n "${DBUS_SESSION_BUS_ADDRESS:-}" ]; then
    "$MOCK" "Smoke Song" "Smoke Band" &
    PIDS+=($!)
    for _ in $(seq 50); do [ "$(STATUS .track)" = "Smoke Band - Smoke Song" ] && break; sleep 0.1; done
    check mpris_track "$([ "$(STATUS .track)" = "Smoke Band - Smoke Song" ] && echo 1 || echo 0)" "track='$(STATUS .track)'"
    # A track change updates the pill quietly: no banner.
    check track_change_no_banner "$([ -z "$(STATUS .banner)" ] && echo 1 || echo 0)" "banner='$(STATUS .banner)'"
    playerctl --player=perchmock play-pause || info "playerctl failed"
    for _ in $(seq 30); do [ "$(STATUS .playing)" = false ] && break; sleep 0.1; done
    check mpris_playerctl_pause "$([ "$(STATUS .playing)" = false ] && echo 1 || echo 0)" "playing=$(STATUS .playing)"
    "$BIN" play-pause
    for _ in $(seq 30); do [ "$(STATUS .playing)" = true ] && break; sleep 0.1; done
    check mpris_cli_resume "$([ "$(STATUS .playing)" = true ] && echo 1 || echo 0)" "playing=$(STATUS .playing)"
    xdotool mousemove $CX $TOP
    sleep 0.8
    import -window root -crop 560x340+$CROPX+$TOP "$OUT/x11-media-$COMP.png" 2>/dev/null || true
    xdotool mousemove $((MX + 40)) $AWAY
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
    xdotool mousemove $CX $TOP
    sleep 0.8
    import -window root -crop 560x340+$CROPX+$TOP "$OUT/x11-clipboard-$COMP.png" 2>/dev/null || true
    xdotool mousemove $((MX + 40)) $AWAY
    sleep 0.8
else
    info "clipboard end-to-end skipped (set PERCH_CLIP_OWNER)"
fi

# Every tab, pinned open, as visual evidence of the panel design.
"$BIN" toggle-notch
sleep 0.8
for t in music tasks clips shelf time tools; do
    "$BIN" tab "$t"
    sleep 0.6
    import -window root -crop 560x340+$CROPX+$TOP "$OUT/x11-tab-$t-$COMP.png" 2>/dev/null || true
done
"$BIN" toggle-notch
sleep 0.8

# Screenshot: open the overlay from the CLI, drag a region, Enter copies it.
"$BIN" shot
for _ in $(seq 50); do [ "$(STATUS .shot_open)" = true ] && break; sleep 0.1; done
check shot_overlay_opens "$([ "$(STATUS .shot_open)" = true ] && echo 1 || echo 0)" "shot_open=$(STATUS .shot_open)"
SHOT=$(xdotool search --name '^Perch screenshot$' | head -1 || true)
if [ -n "$SHOT" ]; then
    xdotool windowactivate --sync "$SHOT" 2>/dev/null || true
    xdotool mousemove $((CX - 260)) $((TOP + 300)) mousedown 1 mousemove $((CX - 160)) $((TOP + 380)) mousemove $((CX - 60)) $((TOP + 450)) mouseup 1
    sleep 0.3
    import -window root -crop 560x320+$CROPX+$((TOP + 260)) "$OUT/x11-shot-$COMP.png" 2>/dev/null || true
    xdotool key --window "$SHOT" Return
    for _ in $(seq 50); do [ "$(STATUS .shot_open)" = false ] && break; sleep 0.1; done
    TARGETS=$(xclip -o -selection clipboard -t TARGETS 2>/dev/null || true)
    check shot_copies_png "$(echo "$TARGETS" | grep -q image/png && echo 1 || echo 0)" "clipboard targets: $(echo $TARGETS | tr '\n' ' ')"
    SIZE=$(xclip -o -selection clipboard -t image/png 2>/dev/null | wc -c)
    check shot_png_nonempty "$([ "$SIZE" -gt 100 ] && echo 1 || echo 0)" "$SIZE bytes"
fi
xdotool mousemove $((MX + 40)) $AWAY

# Auto-hide: over a fullscreen app (EWMH _NET_WM_STATE_FULLSCREEN via the WM)
# or a maximized window the notch rests as a thin stripe, and hovering it
# opens the notch only after 3 s. The victim is Perch's own settings window:
# a normal, focusable top-level.
"$BIN" settings
sleep 1
wmctrl -a "Perch settings" || true
sleep 0.5
wmctrl -r "Perch settings" -b add,fullscreen
sleep 1.2
read -r _ _ SW SH <<<"$(geom)"
check fullscreen_stripes "$([ "$(STATUS .stripe)" = true ] && [ "$(STATUS .visible)" = true ] && [ "$SH" -le 8 ] && echo 1 || echo 0)" "stripe=$(STATUS .stripe), ${SW}x${SH}"
import -window root -crop 560x40+$CROPX+$TOP "$OUT/x11-stripe-$COMP.png" 2>/dev/null || true
# (Over a focused fullscreen window the X11 window manager stacks the app
# above every dock, so the stripe can't be hovered there; the hover reveal is
# checked over a maximized window below, where the stripe stays on top.)
wmctrl -r "Perch settings" -b remove,fullscreen
sleep 1
check unfullscreen_restores "$([ "$(STATUS .stripe)" = false ] && [ "$(STATUS .obscured)" = false ] && echo 1 || echo 0)" "stripe=$(STATUS .stripe)"
wmctrl -r "Perch settings" -b add,maximized_vert,maximized_horz
sleep 1.2
check maximized_stripes "$([ "$(STATUS .stripe)" = true ] && echo 1 || echo 0)" "stripe=$(STATUS .stripe)"
xdotool mousemove $CX $TOP
sleep 1.5
check stripe_hover_waits "$([ "$(STATUS .state)" = Collapsed ] && echo 1 || echo 0)" "state after 1.5 s: $(STATUS .state)"
sleep 2.3
check stripe_hover_reveals "$([ "$(STATUS .state)" = Expanded ] && echo 1 || echo 0)" "state after 3.8 s: $(STATUS .state)"
xdotool mousemove $((MX + 40)) $AWAY
sleep 1.2
check stripe_returns "$([ "$(STATUS .stripe)" = true ] && echo 1 || echo 0)" "stripe=$(STATUS .stripe), state $(STATUS .state)"
wmctrl -r "Perch settings" -b remove,maximized_vert,maximized_horz
sleep 1
check unmaximized_restores "$([ "$(STATUS .stripe)" = false ] && echo 1 || echo 0)" "stripe=$(STATUS .stripe)"
wmctrl -c "Perch settings" || true
sleep 0.5
check screen_time_records "$([ "$(STATUS .usage_apps)" -ge 1 ] 2>/dev/null && echo 1 || echo 0)" "apps today=$(STATUS .usage_apps)"

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
xdotool mousemove $((MX + 40)) $AWAY
sleep 2
RSS_KB=$(awk '/VmRSS/ {print $2}' /proc/"$PID"/status)
T_A=$(awk '{print $14 + $15}' /proc/"$PID"/stat)
sleep 10
T_B=$(awk '{print $14 + $15}' /proc/"$PID"/stat)
TICKS=$(getconf CLK_TCK)
info "VmRSS $((RSS_KB / 1024)) MB; CPU over 10 s idle: $(( (T_B - T_A) * 1000 / TICKS )) ms"
check idle_rss_under_40mb "$([ "$RSS_KB" -lt 40960 ] && echo 1 || echo 0)" "$((RSS_KB / 1024)) MB"
check idle_cpu_near_zero "$([ $((T_B - T_A)) -le 2 ] && echo 1 || echo 0)" "$((T_B - T_A)) ticks in 10 s"
if [ $((T_B - T_A)) -gt 2 ]; then
    # Which thread is busy, for the failure report.
    for t in /proc/"$PID"/task/*; do printf '%s\t%s\n' "${t##*/}" "$(awk '{print $14 + $15}' "$t/stat")"; done >"$TMP/t1"
    sleep 2
    for t in /proc/"$PID"/task/*; do
        a=$(awk -F'\t' -v id="${t##*/}" '$1 == id {print $2}' "$TMP/t1")
        b=$(awk '{print $14 + $15}' "$t/stat")
        [ $((b - ${a:-0})) -gt 0 ] && info "busy thread $(cat "$t/comm"): $((b - ${a:-0})) ticks in 2 s"
    done
    info "last log lines:"; tail -5 "$TMP/perch.log" | tee -a "$RESULTS"
fi

"$BIN" quit
for _ in $(seq 30); do kill -0 "$PID" 2>/dev/null || break; sleep 0.1; done
check quits_cleanly "$(kill -0 "$PID" 2>/dev/null && echo 0 || echo 1)" "perch quit"

echo "--- perch log ---" >>"$RESULTS"
cat "$TMP/perch.log" >>"$RESULTS"
if [ "$FAILS" -gt 0 ]; then
    echo "X11 smoke test FAILED ($FAILS)"; exit 1
fi
echo "X11 smoke test passed ($COMP)"
