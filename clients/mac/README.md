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

Not yet: asks, shared files, notifications, signing out from the Mac, a signed
`.app`, and CI on a macOS runner.
