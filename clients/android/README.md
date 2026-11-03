# Renoa for Android

The Kotlin app shell. Two modules:

- `:rcp` is a client for the RCP JSON/WebSocket binding (`docs/rcp-json-ws-v0.md`, version 16)
  behind the `RcpSurface` interface: discovery with task status, each task's last sequence and
  agent names, a task's journal with optional live text, submitting and stopping a command, and
  the device-key proof, and registering or clearing the device's push token; `PasskeySignIn`
  signs a device in over the coordinator's identity HTTP API (`docs/identity-v0.md`). The app
  passes the signing function; the key itself stays in the platform keystore. Listing and
  revoking signed-in devices are not offered yet. It has no Android dependency and is tested
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
  so `minSdk` is 28. The licences of the fonts, icons and diagram renderer (`licenses/`) ship
  inside the APK as assets.

## Push wake-ups

Renoa wakes a closed app with a content-free Firebase Cloud Messaging message (issue #116,
`crates/renoa-control/src/fcm.rs`); what changed is read over RCP, so nothing an agent wrote
reaches Google. `PushMessages` takes `{"renoa": "changed"}` and `{"renoa": "ring"}`, ignores
anything else, and enqueues `WakeWork`, an expedited WorkManager job. A ring (#86, what an agent
needs from the owner now) first posts "Renoa needs you" on the "Needs you" channel, which sounds the
phone's ringtone and names no agent; a later ring replaces it, and opening the app takes it back.
Home lists the feed it rang for: the app reads it when it connects and on each roster check, not
from the wake job. Push registers while either channel may show
something. Jobs run one after another; one whose wake a later catch-up already covered ends without
connecting.

The job (`WakeUp`) connects with the device credential and key, registers the push token, lists the
tasks, attaches from its own cursor (`WakeCursors`, kept per sign-in in the app's storage) to each
task whose `throughSequence` passed it, closes the connection, and posts one notification per task:
the agent's latest reply as plain words (markdown marks removed, at most 1,000 characters, "New
reply" when no words are left), or that it got stuck, under the name Home keeps for it. A new reply
rings even over an unopened notice from the same chat; the same reply posted again, as on a retry,
does not. A task's cursor moves only once its notice is up. A notice from an agent's chat opens that
chat when tapped, through `NoticeTap`, which is not exported and hands the chat to the app in its
own process, so no other app can open a chat; one from another task, such as a routine's, opens the
app. A chat on screen gets no notice, opening a chat takes its notice back, and what the app had
applied of a chat when it came on screen or left it counts as read, so a later wake does not show
it again; the job checks that, and the screen, right before it posts. Nothing posts after sign-out.
A task, the token or the baseline that failed does not stop the rest; a task that ends the
connection, or finds it already gone, is tried once more on a new one that reads from the kept
cursors afresh, and is left for the next run if it ends that one too. The
job then runs again, a few times at most. A sign-in Renoa refused ends it; one the Keystore could
not read just now is retried.

The first registration of a sign-in records where every task stands, so a new phone is not told its
whole history. If a wake-up has to take it, because the app never could, what was there counts as
history too. A cursor file that does not parse counts as nothing kept: a new baseline is taken, and
records since the last good write are not shown, rather than every chat's history.

Every connection Renoa accepts, the app's own and each job's, sends `set_push_token` while
notifications are allowed and `clear_push_token` once they are not, one at a time so an older answer
never lands after a newer one; a new token from Firebase starts a job. Android 13 and later ask for
the notification permission the first time the owner sends a message. The app uses Firebase
registration tokens, which firebase-messaging 26 deprecates in favour of installation IDs; which of
the two FCM HTTP v1 takes is open in `docs/rcp-v0.md`.

`google-services.json` belongs to the owner's Firebase project and never enters the repository.
The build reads it from the Gradle property `renoa.googleServices`, else the environment
variable `RENOA_GOOGLE_SERVICES`, else `~/.config/renoa/google-services.json`, and writes the four
values Firebase needs as string resources, as the google-services plugin would. A named file that
does not exist is a warning; one that does not parse fails the build, naming the file and never
quoting it. Without the file the app builds, its tests pass, and push is off: the app clears its
token on every connection.

## Build

Needs JDK 21 and the Android SDK (platform `android-37.0`). From this directory:

```sh
./gradlew assembleDebug testDebugUnitTest :rcp:test lint
```

`:rcp:test` runs the RCP client's own tests; `testDebugUnitTest` covers only the app.

The screen tests run under Robolectric and write pictures to `app/build/screens`, so the
screens can be looked at without a phone or emulator.

Versions are pinned in `gradle/libs.versions.toml`. The app compiles against SDK 37 and still
targets 36, and some pinned AndroidX and OkHttp versions trail the newest releases; lint therefore
has the three "newer version available" checks switched off in `app/build.gradle.kts`.

Agent replies render as markdown through `multiplatform-markdown-renderer` (core and Material 3
modules only). An image shows as its description, and only web links are links; they open in the
phone's browser. The Compose BOM is kept on the Compose release that library is built against, so
the app carries one Compose version.

A fenced code block in the `mermaid` language draws as a diagram. Pele (`assets/diagram/`,
provenance in `licenses/README.md`) runs in a web view per diagram that blocks network loads,
reads no files and offers the app no bridge; the page carries Pele, Instrument Sans and the
diagram's source as escaped text, and takes its colours from the theme. It tells the app its
height, or that it cannot draw, through its title. A diagram Pele cannot draw, or one taller than
4,000 dp, shows as its code; one still arriving is a placeholder until its fence closes.
Robolectric runs no script, so the tests give the view the page's title themselves; the page's
own side, that it draws and sets that title, is checked by hand by opening the pages the tests
write to `app/build/diagrams` in Chrome.
