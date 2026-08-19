# Renoa

Renoa is the reference implementation of the Renoa Continuity Protocol (RCP),
a task-centered protocol for continuing agent work across surfaces and
execution environments.

The durable task is Renoa's stable core. Mac, Android, Telegram, GitHub, and
other surfaces attach to the same authorized task journal. Execution nodes
temporarily perform work for that task through a replaceable harness such as
Renoa's Rust kernel, Pi SDK, or a future adapter. Connections, surfaces, nodes,
models, and harnesses may change without changing task identity.

The repository currently contains a standalone Rust Agent SDK, a durable
standalone Rust harness with crash-safe sequential tools, cancellation,
context projection, durable bounded compaction, provider-overflow recovery,
and truthful model-usage inspection, a stable ACP v1 adapter for local coding
frontends, a
kernel-v0 reference executor, a live `renoa-node`
bridge, a headless TypeScript surface client, a Pi SDK execution node, and a
loopback-only continuity proof. Both
execution bridges keep model execution independent of the coordinator socket,
publish locally committed events while a turn is running, and resume from
durable acknowledgement cursors after reconnecting. The coordinator
authenticates enrolled devices, lists each principal's tasks, durably dispatches
owned work, maintains one ordered journal for every surface, and replays missed
events. This is not yet a stable public RCP wire implementation.

Run the loopback coordinator as its own process with:

```sh
cargo run -p renoa-control --bin renoa-coordinator -- \
  serve /absolute/path/to/control.sqlite 7818
```

The process always binds `127.0.0.1`, prints its JSON WebSocket endpoint after
the database and listener are ready, and stops cleanly on `SIGINT` or
`SIGTERM`. It intentionally has no public plaintext mode.

Create a single-use surface enrollment from the trusted host with:

```sh
renoa-coordinator enroll-surface \
  /absolute/path/to/control.sqlite <principal-uuid> <surface-name>
```

The JSON output contains a secret token that expires after five minutes. This
command adds no remote administration endpoint. Provision a node identity and
one task binding from the same trusted host with:

```sh
renoa-coordinator enroll-node \
  /absolute/path/to/control.sqlite <node-uuid>
renoa-coordinator create-task \
  /absolute/path/to/control.sqlite \
  <task-uuid> <principal-uuid> <node-uuid> <target>
```

Node enrollment also emits a single-use five-minute token. `create-task`
persists the exact operator-selected identities and emits nothing on success.
These local bootstrap commands are not part of RCP's network operations.

The first private VPS deployment uses systemd and Tailscale Serve without
exposing the coordinator to the public internet. Its exact operational contract
is in [deploy/README.md](deploy/README.md).

The TypeScript client is in
[clients/typescript](clients/typescript/README.md). It proves authentication,
task discovery and authorization, durable replay cursors, live reattachment,
typed recovery failures, and idempotent command recovery against the real Rust
coordinator without becoming part of the agent kernel.

The Pi node is in [nodes/pi](nodes/pi/README.md). It proves that RCP's shared
execution profile is independent of Renoa's Rust kernel and uses Pi's own
provider routing for OpenCode Go and xAI. Its first capability proof configures
Pi's packaged read, write, and edit tools locally while both Pi and the Rust
kernel consume the same neutral RCP execution delivery. SuperGrok device OAuth,
credential refresh, filesystem paths, prompts, models, tools, and permission
policy all stay outside RCP.

The first standalone coding host is in
[crates/renoa-local](crates/renoa-local/README.md). It runs the durable Rust
harness with Pi AI provider routing and external local read, edit, write, and
bash tools. It proves a real coding turn without coupling provider credentials,
filesystem behavior, or process policy into `renoa-harness`.

The ACP adapter is in [crates/renoa-acp](crates/renoa-acp). A coding frontend
launches `renoa-agent acp`; the adapter creates or reloads the same durable
harness session, exposes standard ACP model and reasoning selectors, streams
model and tool progress, and cancels active work. Its exact supported surface and deliberate limits are in
[docs/acp-v1.md](docs/acp-v1.md). ACP stays outside both the harness and RCP.

## License

Renoa is available under either the
[Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT),
at your option.

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion in Renoa are provided under those same terms without additional
conditions.

Start with [docs/rcp-v0.md](docs/rcp-v0.md), the canonical RCP architecture and
decision record. The transport-independent behavior is in
[docs/rcp-operations-v0.md](docs/rcp-operations-v0.md), and its first concrete
binding is in [docs/rcp-json-ws-v0.md](docs/rcp-json-ws-v0.md). See
[docs/continuity-v0.md](docs/continuity-v0.md) for the current proof,
[docs/identity-v0.md](docs/identity-v0.md) for device trust,
[docs/kernel-v0.md](docs/kernel-v0.md) for the optional reference executor,
[docs/agent-v0.md](docs/agent-v0.md) for the standalone Rust Agent SDK boundary,
[docs/harness-v0.md](docs/harness-v0.md) for the standalone durable harness
and its remaining durable-harness architecture,
[docs/acp-v1.md](docs/acp-v1.md) for the local coding-frontend adapter, and
[docs/reference-implementations.md](docs/reference-implementations.md) for the
upstream designs being studied.
