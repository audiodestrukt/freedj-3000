# Cross-build OpenDeck for a 64-bit Raspberry Pi (aarch64, glibc 2.36 = Pi OS
# Bookworm) from an x86 box.  Only ALSA and SDL2 (the renderer-sdl feature)
# are linked from the system; Rubber Band and Bungee are vendored and compiled
# by build.rs with the cross g++.
#
#   deploy/pi/cross/build.sh            # → target/aarch64-unknown-linux-gnu/release/opendeck
FROM rust:1-bookworm
RUN dpkg --add-architecture arm64 \
 && apt-get update \
 && apt-get install -y --no-install-recommends \
      g++-aarch64-linux-gnu libasound2-dev:arm64 libsdl2-dev:arm64 pkg-config \
 && rm -rf /var/lib/apt/lists/*
# rust-toolchain.toml asks for `stable`; the image ships a version-named
# toolchain, so install `stable` (with the components it lists) once here.
RUN rustup toolchain install stable --profile minimal --component rustfmt,clippy \
 && rustup target add --toolchain stable aarch64-unknown-linux-gnu \
 && chmod -R a+rwX /usr/local/rustup
ENV CARGO_TARGET_AARCH64_UNKNOWN_LINUX_GNU_LINKER=aarch64-linux-gnu-gcc \
    CC_aarch64_unknown_linux_gnu=aarch64-linux-gnu-gcc \
    CXX_aarch64_unknown_linux_gnu=aarch64-linux-gnu-g++ \
    AR_aarch64_unknown_linux_gnu=aarch64-linux-gnu-ar \
    PKG_CONFIG_ALLOW_CROSS=1 \
    PKG_CONFIG_PATH_aarch64_unknown_linux_gnu=/usr/lib/aarch64-linux-gnu/pkgconfig \
    CARGO_HOME=/cargo-home \
    RUSTUP_AUTO_INSTALL=0
WORKDIR /work
