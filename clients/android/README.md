# Renoa for Android

The Kotlin app shell. Two modules:

- `:rcp` is a client for the RCP JSON/WebSocket binding (`docs/rcp-json-ws-v0.md`, version 14)
  behind the `RcpSurface` interface: discovery with task status and agent names, a task's
  journal with optional live text, submitting and stopping a command, and the device-key proof;
  `PasskeySignIn` signs a device in over the coordinator's identity HTTP API (`docs/identity-v0.md`).
  The app passes the signing function; the key itself stays in the platform keystore. Listing
  and revoking signed-in devices are not offered yet. It has no Android dependency and is tested
  against a fake coordinator that speaks the raw wire. Tests cross-check the pinned version and
  every frame shape against the worked examples in that document.
- `:app` is the screens: theme (Graphite and Paper), the agent badge and its five state
  motions, Home, Chat, create/edit agent, and sign-in. Sign-in is real: a setup code (or the
  ID of a passkey the person already has), a passkey through Credential Manager, and a P-256
  device key in the Android Keystore; the device credential is kept encrypted with a Keystore
  key. The app signs in to `BuildConfig.RENOA_ORIGIN`, which must serve `assetlinks.json` for
  this package and signing certificate (`deploy/README.md`). Once signed in, the screens run on
  `RcpAgentRepository`: chat over `:rcp`, and making an agent or reading its profile through
  the Host over the same connection. Tests use `FakeAgentRepository`. Passkeys need Android 9,
  so `minSdk` is 28. The font and icon licences (`licenses/`) ship inside the APK as assets.

## Build

Needs JDK 21 and the Android SDK (platform 36). From this directory:

```sh
./gradlew assembleDebug testDebugUnitTest lint
```

The screen tests run under Robolectric and write pictures to `app/build/screens`, so the
screens can be looked at without a phone or emulator.

Versions are pinned in `gradle/libs.versions.toml`. The newest AndroidX releases need
compileSdk 37, so the app stays on the Compose BOM and AndroidX versions that fit SDK 36; lint
therefore has the three "newer version available" checks switched off in `app/build.gradle.kts`.
