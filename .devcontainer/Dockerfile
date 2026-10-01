# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors

# Linux development container (Ubuntu 22.04 = the glibc floor for Linux builds).
# Windows and macOS developers use `cargo xtask doctor` instead.
FROM mcr.microsoft.com/devcontainers/base:ubuntu-22.04

RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake ninja-build nasm pkg-config git curl ca-certificates \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev \
    && rm -rf /var/lib/apt/lists/*
