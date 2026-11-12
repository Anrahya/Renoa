# Renoa for the Mac

A thin chat with the owner's agents, in GPUI with
[gpui-kit](https://github.com/longbridge/gpui-kit), over the shared
`renoa-rcp-client`. It is its own Cargo workspace with its own `Cargo.lock` and
`target/`, so the Linux build and CI never compile GPUI. It builds on a Mac
with the Command Line Tools; Xcode is not needed.

```sh
cargo run
cargo test
cargo clippy --all-targets -- -D warnings
```

`cargo run` signs the app with a code-signing identity named `Renoa Dev`
when the login Keychain has one (`scripts/run-signed.sh`). Without it, each
build is a new app to the Keychain, which asks for the login password again.
Make it once in Keychain Access: Certificate Assistant → Create a
Certificate…, name `Renoa Dev`, identity type Self Signed Root, certificate
type Code Signing.

To use it as an app, with its icon in the Dock and Launchpad:

```sh
scripts/install-app.sh
```

It builds in release, wraps the binary as `Renoa.app` with the icon drawn from
`assets/icon.svg` (the Renoa mark on a Paper tile, `scripts/bundle-app.sh`),
signs it like `cargo run` does, and installs it in `/Applications`. The
installed app stays the same binary until the next install, so once the
Keychain is told to Always Allow it, it stops asking for the login password;
each `cargo run` rebuild asks again.

## Updates

A release can carry the Mac app: `scripts/publish-mac-app <tag>`, run on the
Mac holding the `Renoa Dev` identity, builds it with the tag's version, zips
it and attaches it to the release; `renoa-deploy` then serves it at
`https://renoa.live/app/mac.json` (`deploy/README.md`). A published app checks
there at launch and whenever Settings opens. When a newer one is ready, a bar
along the bottom of the window and Settings offer **Update and relaunch**: the
app downloads the zip, checks its size and SHA-256 against `mac.json`, checks
that the unzipped app is Renoa at that version signed by the key that signed
the running app, swaps it into place and reopens. Any failure leaves the
running app as it was (`src/update.rs`).

Only a published app updates. One built by `install-app.sh` or `cargo run`
says it was built from source; install a published one once from
`https://renoa.live/app/renoa-mac-<version>.zip`. The Keychain may ask for the
login password once after an update.

The first build takes a few minutes; `target/` grows to several GB, which `cargo clean`
frees.

## Signing in

The Mac has no passkey. On first launch it makes a P-256 key, keeps it in the
login Keychain (service `live.renoa.mac`), and shows an eight-symbol code. In
Renoa on the phone, Settings → Sign in a computer, the owner types the code;
the Mac collects its credential, keeps it in the Keychain beside the key, and
connects with the key (`docs/identity-v0.md`, "Signing in a computer from a
phone"). If Renoa refuses the credential, the Mac forgets it and shows a new
code.

`RENOA_ORIGIN` points the app at another coordinator, such as
`http://127.0.0.1:7818`; the default is `https://renoa.live`.

## What it does

- Agents are the targets the owner's online nodes advertise.
- Each agent wears a face drawn from the look its Host picked
  (`GET /v1/host/agents/<id>/appearance`), the same parts the phone draws
  (`src/face.rs`). The main agent, the one the Host names (`GET /v1/host`) or
  until it names one the first listed, wears Renoa's own look and her star; an
  agent whose look is not read yet wears a plain one. Faces blink from one
  clock and bob while their agent works.
- Under each agent's name the sidebar shows the last thing said, or, before
  anything is, the purpose its owner gave it (`GET /v1/host/agents/<id>`), and
  the time of the last line that has one.
- The Host is read for every `agent:<id>` target beside the chats, so a slow
  Host holds up no message. The main agent is read with every agent list,
  every 30 seconds; a purpose or look is read until the Host gives it or says
  there is no such agent.
- Agent lines show as Markdown on the page itself, without a bubble, with the
  agent's face beside the last of a run; a time goes between lines an hour or a midnight apart. An owner's line
  takes the time the agent began answering it. Lines new when they arrive
  come in with a motion; a replay of older ones does not.
- Each agent's chat is the task every app derives from the agent's node and
  target, so the phone and the Mac share one conversation (`src/chat.rs`).
- A lost connection is retried every five seconds; each chat is attached again
  after the last event the Mac received. A call Renoa leaves unanswered for 30
  seconds counts as a lost connection.
- A message keeps one id until Renoa confirms it, so one sent as the connection
  drops is sent again on the next and admitted once. That holds while the app
  runs: a message still waiting when the app quits is not sent. While Renoa is
  unreachable, Enter leaves the text in the composer.
- A message Renoa refuses returns to its chat's composer, ahead of anything
  typed since.

View → Bigger (⌘=), Smaller (⌘−) and Actual Size (⌥⌘0) draw the whole app
larger or smaller, as a browser zooms a page, from 75% to 250%. Every length is
in the app's rem unit (`theme::u`), so the window's rem size scales it all; the
choice is kept in `settings.json` for the next launch.

## Home

Home shows first, from the sidebar's top row (⌘0), and reads the owner's
feed on the Host (`GET /v1/host/feed`, every page) when it opens and every
minute:

- The day in a line, as the Host words it.
- **Needs you**: each agent's newest question. A choice is sent to the
  agent's chat, opened first if this Mac has not; a question asked in
  another conversation is named in the answer and put away once Renoa takes
  it (`POST /v1/host/feed/items/<id>/dismissal`). The question hides at once
  and shows again if Renoa refuses the answer. While Renoa is out of reach,
  Home does not take an answer.
- **While you were away**: updates, routine results, failed routines and
  abilities turned on, each dismissable. Run again asks under one operation
  id until the Host answers, then puts the note away; the Host opens a new
  one if the routine fails again.
- **Coming up**: the three routines that run next.

The sidebar's Home row says how many things need the owner.

## Sheets and keys

Four sheets rise over the chats from the sidebar's foot, each read from the
owner's Host through the session (`src/sheets/`):

- **Routines** (⌘⇧R): what the agents do on their own, from `GET /v1/host`,
  without the triggers a Host plugin owns, such as reminders. Each shows its
  agent, its schedule in the phone's words, how its last run went and when it
  runs next. A switch pauses or resumes it
  (`POST /v1/host/automations/<id>/enabled`); a receipt or a read shows only
  when it is newer than what is shown. Run now asks for one run
  (`POST /v1/host/automations/<id>/run`) and asks again under the same
  operation id until the Host confirms it. New routine opens the main
  agent's chat.
- **Memory** (⌘⇧M): the owner's profile (`GET /v1/host/profile`), the
  reminders, and the newest notes with the face of the agent that wrote them
  (`GET /v1/host/memory`). It only reads; Tell opens the main agent's chat.
- **Apps** (⌘⇧A): every app any agent can use (`GET
  /v1/host/agents/<id>/apps`), once each, with a chip per agent that turns it
  on or off for that agent (`POST /v1/host/agents/<id>/apps/<connection>/enabled`).
- **Settings** (⌘,): System, Light or Dark, kept for the next launch in
  `~/Library/Application Support/live.renoa.mac/settings.json`; the keys;
  and signing out, which forgets this Mac's credential in the Keychain and
  shows a new code. Renoa itself is not told, so the Mac stays among the
  owner's devices.

An agent's own page rises the same way (`src/sheets/profile.rs`). It opens
from the agent's name at the top of its chat or with ⌘I, for the Host's
agents only. The page shows:

- the agent's face, and what it is for, as the chats already read it;
- how long it has been around (`GET /v1/host/agents/<id>`);
- the routines it runs (`GET /v1/host`);
- the apps it has on (`GET /v1/host/agents/<id>/apps`);
- its four latest notes and questions, read back through the feed's pages
  until four are found (`GET /v1/host/feed`).

If the agent itself cannot be read, the page offers to read again; an agent
the Host no longer has is said to be gone. Each other part that cannot be
read says so on its own line while the rest shows. The page only shows,
because the Host cannot change an agent from an app yet. Message opens the
agent's chat; All routines and Manage open those sheets in its place.

A Host call made while Renoa is out of reach is answered at once as not
reached; it is not made later, when the owner may have moved on.

⌘K opens a box that jumps to Home, an agent or a sheet by part of its
name; ⌘0 goes Home; ⌘1 to ⌘9 open the first nine agents; ⌘I opens the
page of the agent whose chat is open; Esc closes a
sheet or the box. Settings lists the keys, and a test presses each one it
lists and checks it leads where its words say (`src/chats/tests.rs`).

The menu bar (`src/menus.rs`) has four menus:

- **Renoa**: Settings, Services, Hide, Hide Others, Show All, and Quit (⌘Q).
- **Edit**: undo, redo and the clipboard, for the text fields.
- **Go**: Home, Jump To, About This Agent, and the sheets. About This Agent
  is dimmed unless a chat with one of the Host's agents is showing.
- **Window**: Minimize (⌘M), Zoom, and Close Window (⌘W), which quits, since
  the app is its one window.

The keys the menus show are bound to the items' actions, so an item and its
key do the same thing. Esc and ⌘1 to ⌘9 are read as they are pressed. Tests
check that every item but Show All and Zoom has keys bound, and that
Minimize, Zoom and Close Window reach the window.

To look at every screen without a display, in both looks, with made-up
agents and Host answers (`src/chats/screens.rs`):

```sh
RENOA_SCREENS=/some/folder cargo test screens -- --ignored
```

Not yet: asks, shared files, notifications, a signed `.app`, and CI on a
macOS runner.
