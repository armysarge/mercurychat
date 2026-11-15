# MercuryChat M0 lab image (Debian 13 / trixie, same base as Mercury CI).
#
# One image for three jobs:
#   1. build Mercury for linux-amd64, linux-arm64 (cross) and windows-x64 (MinGW cross),
#      following F:\Programming\mercury\.github\workflows\build.yml;
#   2. run two Mercury instances over two crossed PulseAudio null sinks (PTT none);
#   3. host the Linux side of the MercuryChat test client.
#
# The Mercury source is mounted read-only and copied before building, so the pinned
# checkout is never modified.
FROM debian:trixie

ENV DEBIAN_FRONTEND=noninteractive

RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      ca-certificates build-essential git pkg-config file procps \
      libpulse-dev libasound2-dev libhamlib-dev libssl-dev libiniparser-dev \
      gcc-aarch64-linux-gnu \
      libpulse-dev:arm64 libasound2-dev:arm64 libhamlib-dev:arm64 libssl-dev:arm64 \
      gcc-mingw-w64-x86-64 \
      pulseaudio pulseaudio-utils \
 && rm -rf /var/lib/apt/lists/*

WORKDIR /work
