# Renoa for Android

The Kotlin app shell. Two modules:

- `:rcp` is a client for the RCP JSON/WebSocket binding (`docs/rcp-json-ws-v0.md`, version 14)
  behind the `RcpSurface` interface: discovery with task status and agent names, a task's
  journal with optional live text, submitting and stopping a command, and the device-key proof.
  The app passes the signing function; the key itself stays in the platform keystore. Listing
  and revoking signed-in devices are not offered yet. It has no Android dependency and is tested
  against a fake coordinator that speaks the raw wire. Tests cross-check the pinned version and
  every frame shape against the worked examples in that document.
- `:app` is the screens: theme (Graphite and Paper), the agent badge and its five state
  motions, Home, Chat, create/edit agent and a sign-in placeholder. It runs on
  `FakeAgentRepository`; nothing in `:app` calls `:rcp` yet. The real repository arrives with the
  sign-in, streaming-chat and agent-profile work in the Phase 5 issues, and it replaces the
  fake behind `AgentRepository`.

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
