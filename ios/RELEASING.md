# Releasing to TestFlight

The iOS app (**OpenDeck DJ**, bundle `com.audiodestrukt.opendeck`) ships to
TestFlight from GitHub Actions — no Mac needed. Same pattern as the other repos.

## Cut a build

```bash
git tag ios-v0.1.1 && git push origin ios-v0.1.1
```

The tag (`ios-v*`) triggers `.github/workflows/ios.yml` on a `macos-26` runner:
adds the `aarch64-apple-ios` Rust target → archives (the Xcode "Build Rust"
phase compiles the `opendeck-app` staticlib) → cloud-signs and uploads to
TestFlight. You can also run it from the Actions tab (**workflow_dispatch**).

- **Marketing version** comes from the tag: `ios-v0.1.1` → `0.1.1`.
- **Build number** is the Unix epoch (always increasing, never a duplicate), so
  no manual bump — `MARKETING_VERSION` / `CURRENT_PROJECT_VERSION` are injected
  by the workflow (the `Info.plist` references them).

## After the upload

1. **App Store Connect → OpenDeck DJ → TestFlight** — the build shows
   "Processing" for ~5–15 min.
2. Compliance auto-clears (`ITSAppUsesNonExemptEncryption=false` in `Info.plist`).
3. Add yourself under **Internal Testing** (no Beta App Review — instant).
4. Install the **TestFlight** app on the iPad, sign in, and the build appears.

## Zero-click delivery (set up once)

Two one-time toggles turn the loop into: **push a tag → wait → it's on the
iPad**, with no manual attaching or tapping Update. After this, the only wait is
Apple's ~5–15 min processing, which no setting can skip.

1. **Auto-attach the build (App Store Connect).** OpenDeck DJ → TestFlight → your
   Internal group → enable **Automatic distribution**. Every processed build then
   joins the group automatically — no per-build attaching. Two things keep this
   from stalling, both already handled here:
   - **Internal testing only** (testers who are users on the team) — internal
     builds get **no Beta App Review**, so they're live the moment processing
     ends. External groups need review, which breaks "automatic."
   - **Export compliance auto-clears** via `ITSAppUsesNonExemptEncryption=false`
     in `Info.plist` — otherwise every build waits on that prompt.
2. **Auto-install (iPad).** TestFlight app → OpenDeck DJ → turn on **Automatic
   Updates** (and allow TestFlight notifications). New builds download and install
   themselves; just open the app.

## App Store submission

Listing text, review notes, privacy answers and the iPad 13" screenshots are in
`ios/appstore/` (`METADATA.md`; `make shots-appstore` regenerates the PNGs).

## Signing / secrets

Automatic signing with a **persistent Apple Distribution certificate** imported
from a `.p12`, plus an App Store Connect API key for the provisioning profile
and the upload. Five repo secrets (the first three share names/values with
curiate; same Apple team `W527ZN3X52`):

| Secret | What |
|---|---|
| `APP_STORE_CONNECT_P8` | the `AuthKey_*.p8` private key (the real secret) |
| `APP_STORE_CONNECT_KEY_ID` | the key's 10-char ID |
| `APP_STORE_CONNECT_ISSUER_ID` | the account issuer UUID |
| `IOS_P12_BASE64` | base64 of `ios-dist.p12` (distribution cert + key + WWDR G3) |
| `IOS_P12_PASSWORD` | the `.p12` password |

Why the `.p12`: without a distribution identity in a keychain,
`-allowProvisioningUpdates` mints a fresh *cloud-managed* certificate on every
ephemeral runner, and Apple caps distribution certs per team — builds started
failing with "maximum number of certificates" after a handful of runs. The
`.p12` is reused across runs instead. If `IOS_P12_BASE64` is unset the import
step is skipped and the old cloud-managed behaviour applies.

The material lives on the dev box in `~/private_keys/` (mode 600):
`ios-dist.key` (private key, generated locally), `ios-dist.csr` (uploaded to
the portal as **Certificates → + → Apple Distribution**), `ios-dist.cer`
(downloaded from the portal; expires 2027-09-03), `ios-dist.p12` and
`ios-dist.p12.pass`. Rebuild the `.p12` after renewing the certificate:

```bash
openssl pkcs12 -export -legacy -inkey ios-dist.key -in ios-dist.pem \
  -certfile AppleWWDRCAG3.pem -out ios-dist.p12 -passout file:ios-dist.p12.pass
gh secret set IOS_P12_BASE64 --body "$(base64 -w0 ~/private_keys/ios-dist.p12)"
```

(`-legacy` matters: macOS `security import` rejects OpenSSL 3's default
PBES2/AES `.p12` encryption.)

Local backup of the key + issuer lives in `~/private_keys/` on the dev box
(mode 600). The key's App Store Connect role must be **App Manager** or Admin.

## Gotchas (learned the hard way)

- **No track is bundled.** `*.mp3` is gitignored, so a CI checkout has none and
  `bundle-track.sh` ships trackless — the app opens to an empty deck. Good for
  testing the UI + Pro DJ Link, but testers hear nothing until there's an in-app
  way to load a track (or a track you own the rights to is bundled). The
  faceplate photo IS tracked, so it still bundles.
- **Multicast entitlement is off by default** (`FREEDJ_ENTITLEMENTS=""`). Link
  works without it over wired ethernet. Turning it on needs a separate Apple
  approval, so leave it off unless Wi-Fi multicast discovery is actually needed.
- **Bundle id is permanent**; the app name is not — rename freely in App Store
  Connect / `CFBundleDisplayName` without touching the bundle id.
- **App icon** lives in `ios/freedj/Assets.xcassets` (opaque 1024, no alpha —
  uploads are rejected otherwise).
- Setting the issuer secret from the local file: use
  `--body "$(cat ~/private_keys/issuer)"`, not `< file`, or the trailing newline
  ends up in the value and breaks `-authenticationKeyIssuerID`.
