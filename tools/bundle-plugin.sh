#!/bin/sh
# Build OpenDeck as a plugin and install it for the local user (Linux):
# CLAP in ~/.clap, VST3 in ~/.vst3.  Restart the host to pick it up.
set -e
cd "$(dirname "$0")/.."
cargo build --release -p opendeck-plugin
so=target/release/libopendeck_plugin.so
mkdir -p ~/.clap ~/.vst3/OpenDeck.vst3/Contents/x86_64-linux
cp "$so" ~/.clap/OpenDeck.clap
cp "$so" ~/.vst3/OpenDeck.vst3/Contents/x86_64-linux/OpenDeck.so
echo "installed ~/.clap/OpenDeck.clap and ~/.vst3/OpenDeck.vst3"
