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
- Agent lines show as Markdown, with the agent's face beside the last of a
  run; a time goes between lines an hour or a midnight apart. An owner's line
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

An agent's own page rises the same way, from its name at the top of its
chat or ⌘I (`src/sheets/profile.rs`). It shows the agent's face, what it is
for and how long it has been around (`GET /v1/host/agents/<id>`), the
routines it runs (`GET /v1/host`), the apps it has on (`GET
/v1/host/agents/<id>/apps`), and its four latest notes and questions from
the feed's newest page (`GET /v1/host/feed`). If any of those reads fails,
the page says so and offers to read again. The page only shows, because the
Host cannot change an agent from an app yet. Message opens the agent's chat;
All routines and Manage open those sheets in its place.

A Host call made while Renoa is out of reach is answered at once as not
reached; it is not made later, when the owner may have moved on.

⌘K opens a box that jumps to Home, an agent or a sheet by part of its
name; ⌘0 goes Home; ⌘1 to ⌘9 open the first nine agents; ⌘I opens the
page of the agent whose chat is open; Esc closes a
sheet or the box. Settings lists the keys, and a test presses each one it
lists (`src/chats/keys.rs`).

To look at every screen without a display, in both looks, with made-up
agents and Host answers (`src/chats/screens.rs`):

```sh
RENOA_SCREENS=/some/folder cargo test screens -- --ignored
```

Not yet: asks, shared files, notifications, a signed `.app`, and CI on a
macOS runner.
