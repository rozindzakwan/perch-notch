#!/usr/bin/env bash
# Start a private PulseAudio server (null output) and run the per-app volume
# integration test against it. Used by scripts/linux-ci.sh (stage "pulse").
set -euo pipefail
cd "$(dirname "$0")/.."

export XDG_RUNTIME_DIR
XDG_RUNTIME_DIR=$(mktemp -d)
chmod 700 "$XDG_RUNTIME_DIR"
export HOME
HOME=$(mktemp -d)

# --exit-idle-time=-1: keep running between streams. Running as root in a
# container prints a warning; it still works for a single-user test.
pulseaudio --daemonize=yes --exit-idle-time=-1 --log-target=stderr --disallow-exit 2>/dev/null || true
for _ in $(seq 50); do pactl info >/dev/null 2>&1 && break; sleep 0.1; done
pactl info | grep -E "Server Name|Default Sink" || { echo "pulseaudio did not start" >&2; exit 1; }

status=0
cargo test -p perch-platform-linux --all-features --locked --test pulse -- --include-ignored --test-threads=1 || status=$?
pulseaudio --kill 2>/dev/null || true
exit $status
