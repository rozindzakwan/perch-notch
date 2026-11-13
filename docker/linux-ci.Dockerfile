# Linux CI image for Perch.
#
# Ubuntu 22.04 = the Linux Mint 21.x base (glibc 2.35), so binaries and the
# .deb built here run on Mint 21 and 22. The same steps run on GitHub Actions'
# ubuntu-22.04 runner via scripts/linux-ci.sh; this image lets them run locally
# on a Windows machine through Docker.
#
#   docker build -t perch-linux-ci -f docker/linux-ci.Dockerfile docker
#   docker run --rm -v "$PWD:/src" -v perch-target:/target -v perch-cargo:/root/.cargo/registry \
#       perch-linux-ci scripts/linux-ci.sh all
FROM ubuntu:22.04

ENV DEBIAN_FRONTEND=noninteractive \
    LANG=C.UTF-8 \
    RUSTUP_HOME=/opt/rustup \
    CARGO_HOME=/opt/cargo \
    CARGO_TARGET_DIR=/target \
    PATH=/opt/cargo/bin:$PATH

# Build deps (mirrors scripts/mint-setup.sh) + headless test tooling.
RUN apt-get update && apt-get install -y --no-install-recommends \
        build-essential pkg-config curl ca-certificates git cmake clang libclang-dev \
        libx11-dev libxcb1-dev libxkbcommon-dev libxkbcommon-x11-dev libwayland-dev \
        libpulse-dev libasound2-dev libv4l-dev \
        xvfb openbox xdotool x11-utils x11-xserver-utils imagemagick xclip \
        dbus dbus-x11 playerctl picom \
        sway \
        pulseaudio pulseaudio-utils \
        sysstat procps psmisc file dpkg-dev fakeroot \
        tesseract-ocr \
    && rm -rf /var/lib/apt/lists/*

RUN curl -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain stable \
        --component clippy,rustfmt \
    && cargo install --locked cargo-deb \
    && rm -rf /opt/cargo/registry

WORKDIR /src

# Smoke-test helpers (separate layer so the big apt layer above stays cached).
RUN apt-get update && apt-get install -y --no-install-recommends \
        jq wmctrl x11-apps stalonetray \
    && rm -rf /var/lib/apt/lists/*
