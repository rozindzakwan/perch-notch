#!/usr/bin/env bash
# Perch - the Linux CI steps, in one place.
#
# GitHub Actions (ubuntu-22.04) and the local Docker image
# (docker/linux-ci.Dockerfile) both run exactly this script, so a green local
# container run means the same thing as a green CI run.
#
# Usage: scripts/linux-ci.sh [stage...]
#   stages: check test build smoke mpris pulse wayland deb debinstall soak all
#   (default: all = check test build smoke mpris pulse wayland deb)
set -euo pipefail

cd "$(dirname "$0")/.."
ROOT=$PWD
OUT=${PERCH_CI_OUT:-$ROOT/artifacts/linux}
mkdir -p "$OUT"
export CARGO_TERM_COLOR=${CARGO_TERM_COLOR:-never}
export RUST_BACKTRACE=1

say() { printf '\n==> %s\n' "$*"; }

target_dir() { echo "${CARGO_TARGET_DIR:-$ROOT/target}"; }

stage_check() {
    say "cargo fmt --check"
    cargo fmt --all --check
    say "cargo clippy (default features)"
    cargo clippy --workspace --all-targets --locked -- -D warnings
    say "cargo clippy (all features)"
    cargo clippy --workspace --all-targets --all-features --locked -- -D warnings
}

stage_test() {
    say "cargo test (all features)"
    cargo test --workspace --all-features --locked
}

stage_build() {
    say "cargo build --release"
    cargo build --release --locked -p perch
    local bin
    bin="$(target_dir)/release/perch"
    ls -l "$bin"
    file "$bin" || true
    stat -c '%s' "$bin" >"$OUT/binary-size.txt"
    # glibc floor: the highest GLIBC_x.y symbol version the binary needs.
    objdump -T "$bin" | grep -o 'GLIBC_[0-9.]*' | sort -Vu | tail -1 | tee "$OUT/glibc-floor.txt"
}

stage_smoke() {
    say "X11 smoke test (Xvfb + openbox), with and without a compositor"
    PERCH_BIN="$(target_dir)/release/perch" scripts/smoke-x11.sh --out "$OUT" --compositor none
    PERCH_BIN="$(target_dir)/release/perch" scripts/smoke-x11.sh --out "$OUT" --compositor picom
}

stage_mpris() {
    say "MPRIS integration test on a private session bus"
    dbus-run-session -- cargo test -p perch-platform-linux --all-features --locked \
        --test mpris -- --include-ignored --test-threads=1
}

stage_pulse() {
    say "PulseAudio per-app volume test against a null sink"
    scripts/pulse-test.sh
}

stage_wayland() {
    say "wlr-layer-shell smoke test under headless Sway"
    PERCH_BIN="$(target_dir)/release/perch" scripts/smoke-wayland.sh --out "$OUT"
}

stage_deb() {
    say "cargo deb"
    cargo deb -p perch --no-build --locked -o "$OUT/"
    ls -l "$OUT"/*.deb
    dpkg-deb --info "$OUT"/perch_*.deb | tee "$OUT/deb-info.txt"
}

stage_debinstall() {
    say "install the .deb into this (clean) system and run --version"
    apt-get update -qq
    apt-get install -y -qq "$OUT"/perch_*.deb
    perch --version
    test -f /usr/share/applications/perch.desktop
    apt-get remove -y -qq perch
}

stage_soak() {
    say "soak test (${PERCH_SOAK_MINUTES:-30} min)"
    PERCH_BIN="$(target_dir)/release/perch" scripts/soak-linux.sh --out "$OUT" \
        --minutes "${PERCH_SOAK_MINUTES:-30}"
}

stages=("$@")
if [ ${#stages[@]} -eq 0 ] || [ "${stages[0]}" = all ]; then
    stages=(check test build smoke mpris pulse wayland deb)
fi

for s in "${stages[@]}"; do
    if ! declare -F "stage_$s" >/dev/null; then
        echo "unknown stage: $s" >&2
        exit 2
    fi
    "stage_$s"
done
say "linux-ci: ${stages[*]} passed"
