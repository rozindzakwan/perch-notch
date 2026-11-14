#!/usr/bin/env bash
# Long-running resource soak test for Perch on Linux (X11).
#
# Starts Xvfb + openbox unless --real-display is given (then uses $DISPLAY,
# e.g. a real Cinnamon session), launches Perch with the everyday features on,
# samples every 10 s: VmRSS, CPU ticks, open fds, threads; toggles the notch
# via the CLI every 5 minutes. Writes soak-linux.csv and soak-linux.md to
# --out and fails on growth beyond the thresholds.
#
# Usage: scripts/soak-linux.sh [--out DIR] [--minutes N] [--real-display]
#                              [--warmup SECONDS] [--summarize CSV]
set -euo pipefail

OUT=artifacts/linux
MINUTES=30
REAL=0
WARMUP=120
SUMMARIZE=""
while [ $# -gt 0 ]; do
    case "$1" in
        --out) OUT=$2; shift 2 ;;
        --minutes) MINUTES=$2; shift 2 ;;
        --real-display) REAL=1; shift ;;
        --warmup) WARMUP=$2; shift 2 ;;
        --summarize) SUMMARIZE=$2; shift 2 ;;  # re-score an existing CSV
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
mkdir -p "$OUT"
OUT=$(cd "$OUT" && pwd)
TICKS=$(getconf CLK_TCK)
if [ -n "$SUMMARIZE" ]; then
    CSV=$SUMMARIZE
else
BIN=${PERCH_BIN:-target/release/perch}
BIN=$(cd "$(dirname "$BIN")" && pwd)/$(basename "$BIN")
TMP=$(mktemp -d)
PIDS=()
cleanup() {
    for p in "${PIDS[@]:-}"; do kill "$p" 2>/dev/null || true; done
    for p in "${PIDS[@]:-}"; do wait "$p" 2>/dev/null || true; done
    rm -rf "$TMP"
}
trap cleanup EXIT

if [ "$REAL" = 0 ]; then
    N=97
    while [ -e "/tmp/.X$N-lock" ]; do N=$((N + 1)); done
    export DISPLAY=:$N
    Xvfb "$DISPLAY" -screen 0 1920x1080x24 -nolisten tcp >/dev/null 2>&1 &
    PIDS+=($!)
    for _ in $(seq 50); do xdpyinfo >/dev/null 2>&1 && break; sleep 0.1; done
    openbox >/dev/null 2>&1 &
    PIDS+=($!)
    sleep 1
fi
export XDG_RUNTIME_DIR="$TMP/run" XDG_CONFIG_HOME="$TMP/config" XDG_DATA_HOME="$TMP/data" XDG_CACHE_HOME="$TMP/cache"
mkdir -p "$XDG_RUNTIME_DIR" && chmod 700 "$XDG_RUNTIME_DIR"
cat >"$TMP/config.toml" <<CFG
[features]
media = true
todo = true
pomodoro = true
clipboard = true
shelf = true
reminders = true
screen_time = true
CFG

"$BIN" --config "$TMP/config.toml" >"$TMP/perch.log" 2>&1 &
PID=$!
PIDS+=("$PID")
sleep 5
CSV="$OUT/soak-linux.csv"
echo "t_s,rss_kb,cpu_ticks,fds,threads" >"$CSV"
START=$(date +%s)
NEXT_TOGGLE=300
while :; do
    NOW=$(( $(date +%s) - START ))
    [ "$NOW" -ge $((MINUTES * 60)) ] && break
    kill -0 "$PID" 2>/dev/null || { echo "perch exited during the soak" >&2; cat "$TMP/perch.log"; exit 1; }
    RSS=$(awk '/VmRSS/ {print $2}' /proc/"$PID"/status)
    CPU=$(awk '{print $14 + $15}' /proc/"$PID"/stat)
    FDS=$(ls /proc/"$PID"/fd | wc -l)
    THR=$(awk '/Threads/ {print $2}' /proc/"$PID"/status)
    echo "$NOW,$RSS,$CPU,$FDS,$THR" >>"$CSV"
    if [ "$NOW" -ge "$NEXT_TOGGLE" ]; then
        "$BIN" toggle-notch; sleep 2; "$BIN" toggle-notch
        NEXT_TOGGLE=$((NEXT_TOGGLE + 300))
    fi
    sleep 10
done

fi

# --- summary ----------------------------------------------------------------
# Leak check on medians of 5-minute windows after a warm-up (features fill
# their caches first), CPU gated on the median 10 s interval (a busy loop
# raises every interval) plus an average ceiling.
summarize() {
    awk -F, -v warm="$WARMUP" -v ticks="$TICKS" '
    function median(a, n,   i, j, t) {
        for (i = 2; i <= n; i++) { t = a[i]; for (j = i - 1; j >= 1 && a[j] > t; j--) a[j + 1] = a[j]; a[j + 1] = t }
        return n ? a[int((n + 1) / 2)] : 0
    }
    NR > 1 { n++; T[n] = $1; R[n] = $2; C[n] = $3; F[n] = $4; H[n] = $5 }
    END {
        s = 0; for (i = 1; i <= n; i++) if (T[i] >= warm) { s = i; break }
        if (!s) s = (n > 1) ? 2 : 1
        t0 = T[s]; tl = T[n]; rmax = 0
        for (i = s; i <= n; i++) {
            if (R[i] > rmax) rmax = R[i]
            if (T[i] < t0 + 300) { rs[++nrs] = R[i]; fs[++nfs] = F[i] }
            if (T[i] >= tl - 300) { re[++nre] = R[i]; fe[++nfe] = F[i] }
            if (i > s && T[i] > T[i - 1]) cpu[++nc] = 100 * (C[i] - C[i - 1]) / ticks / (T[i] - T[i - 1])
        }
        el = tl - t0
        avg = el > 0 ? 100 * (C[n] - C[s]) / ticks / el : 0
        printf "%d %d %d %d %d %d %d %d %.3f %.3f %.2f %d %d\n", median(rs, nrs), median(re, nre), rmax, \
            median(fs, nfs), median(fe, nfe), H[s], H[n], el, median(cpu, nc), avg, (C[n] - C[s]) / ticks, n, tl
    }' "$1"
}
read -r RS RE RMAX FS FE HS HE ELAPSED CPU_MED CPU_AVG CPU_S NSAMPLES TLAST < <(summarize "$CSV")
RSS_GROWTH_KB=$((RE - RS))
FAIL=""
[ "$RSS_GROWTH_KB" -gt 5120 ] && FAIL="$FAIL RSS grew $((RSS_GROWTH_KB / 1024)) MB;"
[ $((FE - FS)) -gt 20 ] && FAIL="$FAIL fds grew $((FE - FS));"
awk -v p="$CPU_MED" 'BEGIN { exit !(p > 0.5) }' && FAIL="$FAIL median CPU ${CPU_MED}% of a core;"
awk -v p="$CPU_AVG" 'BEGIN { exit !(p > 2) }' && FAIL="$FAIL average CPU ${CPU_AVG}% of a core;"
RESULT="PASS (thresholds: RSS +5 MB, fds +20, median CPU 0.5% and average CPU 2% of a core)"
[ -n "$FAIL" ] && RESULT="FAIL:$FAIL"
ENVIRONMENT=${SOAK_ENV:-$( [ "$REAL" = 1 ] && echo "real display $DISPLAY (${XDG_CURRENT_DESKTOP:-unknown desktop})" || echo "Xvfb + openbox" )}
cat >"$OUT/soak-linux.md" <<MD
# Perch soak test (Linux)

- Duration: $(awk -v t="$TLAST" 'BEGIN { printf "%.1f", t / 60 }') min, $NSAMPLES samples every 10 s, notch toggled via CLI every 5 min; warm-up ${WARMUP} s excluded
- Start/End = median of the first/last 5 minutes after warm-up
- Environment: $ENVIRONMENT, $(. /etc/os-release; echo "$PRETTY_NAME"), $(nproc) CPUs

| Metric | Start | End | Max | Growth |
|---|---|---|---|---|
| VmRSS (MB) | $(awk -v k="$RS" 'BEGIN { printf "%.1f", k / 1024 }') | $(awk -v k="$RE" 'BEGIN { printf "%.1f", k / 1024 }') | $(awk -v k="$RMAX" 'BEGIN { printf "%.1f", k / 1024 }') | $(awk -v k="$RSS_GROWTH_KB" 'BEGIN { printf "%.1f", k / 1024 }') |
| Open fds | $FS | $FE | | $((FE - FS)) |
| Threads | $HS | $HE | | |

- CPU at rest (median of 10 s intervals): ${CPU_MED}% of one core
- CPU average: ${CPU_S} s over ${ELAPSED} s = ${CPU_AVG}% of one core
- Result: $RESULT
MD
cat "$OUT/soak-linux.md"
[ -z "$FAIL" ]
