#!/usr/bin/env bash
# Build the OpenDeck DJ APK: cargo-ndk for the Rust library, then the SDK's own
# aapt2 / zipalign / apksigner.  No Gradle, no Java: the app is a NativeActivity
# (see AndroidManifest.xml and crates/app/src/android.rs).
#
#   deploy/android/build.sh [VERSION] [ABI...]
#   deploy/android/build.sh 0.4.0                    # arm64-v8a + x86_64
#   deploy/android/build.sh 0.0.0-dev x86_64         # emulator only, faster
#
# Needs ANDROID_HOME (default ~/Android/Sdk) with build-tools + a platform, an
# NDK (ANDROID_NDK_HOME, default the newest under $ANDROID_HOME/ndk), cargo-ndk,
# and the rust targets aarch64-linux-android / x86_64-linux-android.
#
# Signing, first match wins:
#   ANDROID_KEYSTORE + ANDROID_KEYSTORE_PASS (+ ANDROID_KEY_ALIAS, default "opendeck")
#   ~/private_keys/android-release.jks + android-release.pass
#   the SDK debug key (~/.android/debug.keystore, created if missing). An APK
#   signed with it will not install over one signed with the release key.
#
# Output: dist/OpenDeck-DJ-<version>-android.apk (path printed on stdout last
# and written to $GITHUB_OUTPUT as apk=... when run in Actions).
set -euo pipefail

ROOT="$(cd "$(dirname "$0")/../.." && pwd)"
HERE="$ROOT/deploy/android"
VERSION="${1:-0.0.0-dev}"; shift || true
ABIS=("$@"); [ ${#ABIS[@]} -gt 0 ] || ABIS=(arm64-v8a x86_64)
MIN_SDK=26        # AAudio; Android 8.0
TARGET_SDK=34

export ANDROID_HOME="${ANDROID_HOME:-$HOME/Android/Sdk}"
if [ -z "${ANDROID_NDK_HOME:-}" ]; then
  ANDROID_NDK_HOME="$(ls -d "$ANDROID_HOME"/ndk/* 2>/dev/null | sort -V | tail -1)"
fi
export ANDROID_NDK_HOME
[ -d "$ANDROID_NDK_HOME" ] || { echo "no NDK (set ANDROID_NDK_HOME)" >&2; exit 1; }
BT="$(ls -d "$ANDROID_HOME"/build-tools/* | sort -V | tail -1)"
PLATFORM="$(ls -d "$ANDROID_HOME"/platforms/android-* | sort -V | tail -1)"
STRIP="$(ls "$ANDROID_NDK_HOME"/toolchains/llvm/prebuilt/*/bin/llvm-strip | head -1)"
echo "NDK $(basename "$ANDROID_NDK_HOME"), build-tools $(basename "$BT"), $(basename "$PLATFORM")" >&2

# versionCode must only grow; seconds since the epoch, like the iOS build number.
VERSION_CODE="${ANDROID_VERSION_CODE:-$(date +%s)}"

OUT="$ROOT/build/android"
rm -rf "$OUT"; mkdir -p "$OUT/stage/lib" "$OUT/assets" "$ROOT/dist"

# ── Rust library, one per ABI ────────────────────────────────────────────────
for abi in "${ABIS[@]}"; do
  case "$abi" in
    arm64-v8a) triple=aarch64-linux-android ;;
    x86_64)    triple=x86_64-linux-android ;;
    *) echo "unsupported ABI $abi" >&2; exit 1 ;;
  esac
  ( cd "$ROOT" && OPENDECK_VERSION="$VERSION" cargo ndk -t "$abi" -P "$MIN_SDK" \
      rustc -p opendeck-app --lib --release --crate-type cdylib ) >&2
  mkdir -p "$OUT/stage/lib/$abi"
  "$STRIP" --strip-unneeded -o "$OUT/stage/lib/$abi/libopendeck_app.so" \
      "$ROOT/target/$triple/release/libopendeck_app.so"
  ls -l "$OUT/stage/lib/$abi/libopendeck_app.so" >&2
done

# ── Resources, manifest, assets ──────────────────────────────────────────────
# The demo track: an original production by the author, also in the iOS app.
cp "$ROOT/amplificate.mp3" "$OUT/assets/"
"$BT/aapt2" compile --dir "$HERE/res" -o "$OUT/res.zip"
"$BT/aapt2" link -o "$OUT/base.apk" \
  -I "$PLATFORM/android.jar" \
  --manifest "$HERE/AndroidManifest.xml" \
  --min-sdk-version "$MIN_SDK" --target-sdk-version "$TARGET_SDK" \
  --version-code "$VERSION_CODE" --version-name "$VERSION" \
  -A "$OUT/assets" -0 mp3 \
  "$OUT/res.zip"
( cd "$OUT/stage" && zip -qr "$OUT/base.apk" lib )

# ── Align + sign ─────────────────────────────────────────────────────────────
"$BT/zipalign" -f -P 16 4 "$OUT/base.apk" "$OUT/aligned.apk"

ALIAS="${ANDROID_KEY_ALIAS:-opendeck}"
if [ -n "${ANDROID_KEYSTORE:-}" ]; then
  KS="$ANDROID_KEYSTORE"; KSPASS="env:ANDROID_KEYSTORE_PASS"; KIND=release
elif [ -f "$HOME/private_keys/android-release.jks" ]; then
  KS="$HOME/private_keys/android-release.jks"; KSPASS="file:$HOME/private_keys/android-release.pass"; KIND=release
else
  KS="$HOME/.android/debug.keystore"; KSPASS="pass:android"; ALIAS=androiddebugkey; KIND=debug
  if [ ! -f "$KS" ]; then
    mkdir -p "$HOME/.android"
    keytool -genkeypair -keystore "$KS" -storepass android -keypass android -alias androiddebugkey \
      -keyalg RSA -keysize 2048 -validity 10000 -dname "CN=Android Debug,O=Android,C=US" >&2
  fi
  echo "WARNING: signing with the debug key (no release keystore found)" >&2
fi
APK="$ROOT/dist/OpenDeck-DJ-$VERSION-android.apk"
"$BT/apksigner" sign --ks "$KS" --ks-pass "$KSPASS" --ks-key-alias "$ALIAS" \
  --out "$APK" "$OUT/aligned.apk"
"$BT/apksigner" verify "$APK"
echo "signed ($KIND key), versionCode $VERSION_CODE, $(du -h "$APK" | cut -f1)" >&2
[ -n "${GITHUB_OUTPUT:-}" ] && echo "apk=$APK" >> "$GITHUB_OUTPUT"
echo "$APK"
