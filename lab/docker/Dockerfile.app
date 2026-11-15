# MercuryChat desktop build image for ONE Linux target architecture
# (WebKitGTK -dev packages are not co-installable across architectures).
#   docker build -f Dockerfile.app --build-arg ARCH=amd64 -t mercurychat-app-build:amd64 .
#   docker build -f Dockerfile.app --build-arg ARCH=arm64 -t mercurychat-app-build:arm64 .
# Debian 13 / trixie = the Raspberry Pi OS (trixie) baseline. The Vue bundle is
# built on the host and passed in prebuilt, so no Node.js is needed here.
# Cross-built arm64 output is a packaging signal only (docs/07 §6).
FROM debian:trixie
ARG ARCH=amd64
ENV DEBIAN_FRONTEND=noninteractive
RUN set -e; \
    if [ "$ARCH" = arm64 ]; then dpkg --add-architecture arm64; S=":arm64"; X="gcc-aarch64-linux-gnu g++-aarch64-linux-gnu"; else S=""; X=""; fi; \
    apt-get update; \
    apt-get install -y --no-install-recommends ca-certificates curl build-essential pkg-config file git xz-utils $X \
      libwebkit2gtk-4.1-dev$S librsvg2-dev$S libssl-dev$S libxdo-dev$S; \
    rm -rf /var/lib/apt/lists/*

ENV RUSTUP_HOME=/opt/rustup CARGO_HOME=/opt/cargo PATH=/opt/cargo/bin:$PATH
RUN curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --default-toolchain 1.100.0 \
 && rustup target add aarch64-unknown-linux-gnu \
 && cargo install tauri-cli --version "^2.12" --locked

ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++
WORKDIR /src
