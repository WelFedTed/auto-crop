# SPDX-License-Identifier: MIT OR Apache-2.0
# SPDX-FileCopyrightText: 2026 Auto Crop contributors

# Linux development container (Ubuntu 22.04 = the glibc floor for Linux builds).
# Windows and macOS developers use `cargo xtask doctor` instead.
FROM mcr.microsoft.com/devcontainers/base:ubuntu-22.04

# Build toolchain (CMake, NASM, Ninja, WebKitGTK for the shell) plus the M1 dev tools (ROADMAP
# M1.79): Valgrind, ImageMagick and unpaper from Ubuntu, and Tesseract 5.x from the widely used
# alex-p PPA because Ubuntu 22.04 itself ships Tesseract 4.1 (the OCR oracle needs 5.x). Python 3.12
# comes from the devcontainer feature (22.04 ships 3.10; the hashed oracle lock needs 3.12).
RUN apt-get update \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        software-properties-common \
    && add-apt-repository -y ppa:alex-p/tesseract-ocr5 \
    && DEBIAN_FRONTEND=noninteractive apt-get install -y --no-install-recommends \
        build-essential cmake ninja-build nasm pkg-config git curl ca-certificates \
        libwebkit2gtk-4.1-dev libgtk-3-dev libayatana-appindicator3-dev librsvg2-dev libssl-dev \
        valgrind imagemagick unpaper tesseract-ocr tesseract-ocr-eng \
    && rm -rf /var/lib/apt/lists/*
