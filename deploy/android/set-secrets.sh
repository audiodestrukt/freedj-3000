#!/usr/bin/env bash
# Put the Android release signing key on the GitHub repo, for android.yml.
# Reads ~/private_keys/android-release.{jks,pass} straight into `gh secret
# set` on stdin: nothing is printed or left in shell history.
#
#   deploy/android/set-secrets.sh [owner/repo]
set -euo pipefail
REPO="${1:-audiodestrukt/freedj-3000}"
KS="$HOME/private_keys/android-release.jks"
PASS="$HOME/private_keys/android-release.pass"
for f in "$KS" "$PASS"; do [ -f "$f" ] || { echo "missing $f" >&2; exit 1; }; done
base64 -w0 "$KS" | gh secret set ANDROID_KEYSTORE_BASE64 -R "$REPO"
tr -d '\n' < "$PASS" | gh secret set ANDROID_KEYSTORE_PASSWORD -R "$REPO"
gh secret list -R "$REPO" | grep ANDROID_
