# Renoa continuity layer v0

> This document describes the current loopback coordinator and its first public
> TLS deployment. The canonical
> protocol direction, locked decisions, open decisions, and conformance target
> live in [rcp-v0.md](rcp-v0.md). The proven semantics are recorded in
> [rcp-operations-v0.md](rcp-operations-v0.md), and the current wire shape is in
> [rcp-json-ws-v0.md](rcp-json-ws-v0.md). Neither is a stable public release yet.

## Outcome

A task remains addressable when a surface disconnects. Any authorized surface
can discover its tasks, observe the same ordered history, and submit the next
command. Execution still happens on the node that owns the task's target
environment.

Surface handoff means that a separately enrolled device attaches to this same
task with its own credential and cursor. It does not copy another surface's
credential or move the execution node's harness, workspace, or service secrets.

The proof uses Rust surfaces, a headless TypeScript surface, a Rust kernel node,
a Pi SDK node, and one coordinator.

## Core model

The durable task is the stable center of the system:

```text
surface ─┐                       ┌─ execution node
surface ─┼─ coordinator ─ task ─┤
adapter ─┘                       └─ future execution node
```

- A **surface** accepts user input and renders task events. It does not run the
  agent loop or decide execution permissions.
- A **task** is a durable, ordered journal plus an execution binding.
- A **node** owns an environment and runs a selected harness there. The current
  proofs use Renoa's Rust kernel and Pi SDK as independent executors.
- The **coordinator** admits commands, persists task events, routes work to the
  bound node, and replays missed events. It does not call models or execute
  capabilities.
- A **connection** is temporary. Losing one must not change task identity or
  erase task state.

This is not a remote-control tunnel. A tunnel makes a live process the center;
Renoa makes the task journal the center and treats processes as attachments.

## v0 invariants

1. A surface-generated command ID is admitted at most once.
2. A connection's principal and role come from an enrolled device credential,
   never from client-asserted identity fields.
3. A surface may discover only its principal's tasks and may attach to or
   submit work only for an owned task.
4. The coordinator atomically persists a command, its task event, and a pending
   execution delivery before acknowledging it or routing it.
5. Only the node bound to a task may acknowledge execution admission or publish
   execution events for that task.
6. The node durably admits a command in its local harness ledger before
   acknowledging it. The coordinator retains the pending delivery until that
   acknowledgement is committed.
7. The coordinator accepts only contiguous execution-event batches for the
   command's bound execution. It persists each new event and the accepted
   execution cursor before broadcasting or acknowledging them.
8. Every task event receives one monotonically increasing task sequence.
9. A reconnecting surface supplies its last sequence and receives every later
   event exactly once in the reconstructed view.
10. Network delivery is at least once. Stable command and event IDs make retries
   harmless; Renoa does not claim impossible end-to-end exactly-once execution.
11. A slow surface may be disconnected. It recovers from the durable journal
   instead of forcing execution to wait for its socket.
12. If the bound node is offline when a new command is submitted, v0 rejects
    the command explicitly. It must not execute unexpectedly hours later.
13. A retry of an already admitted command remains accepted if its node later
    goes offline. Availability cannot change a durable admission result.
14. A replay cursor ahead of durable task history is rejected instead of being
    treated as a valid empty suffix.

## Ownership

The coordinator owns:

- task identity and its node binding;
- task ownership and enrolled device identity;
- owner-filtered task discovery;
- command admission;
- the cross-surface task sequence;
- connected-node presence;
- replay of the task journal to trusted surfaces for the same principal.

It does not own harness prompts, models, tools, or permission policy.

The node owns:

- the target environment and local path resolution;
- its task-to-harness binding, harness configuration, and capability host;
- model credentials and provider traffic;
- the selected harness ledger and conversation context;
- the durable task/command/execution mapping and coordinator-acknowledged
  source cursor used by its bridge;
- side effects and their local safety policy;
- secure storage of its device credential.

The surface owns:

- capture and presentation;
- secure storage of its device credential;
- a durable command ID until admission is acknowledged;
- its last applied task sequence.

The TypeScript reference surface persists command input before transmission and
removes it only after `command_accepted`. It invokes the host projection before
advancing the durable cursor, so a failed projection is replayed rather than
silently skipped.

## Transport

The version 8 binding uses JSON messages over WebSocket on localhost. WebSocket
supplies an ordered, bidirectional byte stream and works from Rust, TypeScript,
browsers, and mobile clients. JSON keeps the contract inspectable while it is
changing. Exact frames are documented in
[rcp-json-ws-v0.md](rcp-json-ws-v0.md).

Every durable execution task record also carries its causing command identity,
so a reconnecting surface can rebuild concurrent or interleaved turns without
guessing from journal adjacency.

Transport choices are not task semantics. A later Telegram webhook, HTTP API,
or ACP adapter can call the same coordinator operations without becoming part
of the kernel.

The coordinator server is deliberately loopback-only. Device authentication
protects identity but its listener is plaintext, so it refuses a non-loopback
address. The first public deployment uses an outbound Cloudflare Tunnel to
terminate TLS at `wss://renoa.live/connect`. Credentials never travel over
public `ws://` or in a URL.

## Delivery flow

1. A device exchanges one unexpired, single-use enrollment for a credential.
2. A node opens an outbound connection and authenticates; the coordinator
   derives its stable node ID from the enrolled device record.
3. A surface authenticates and lists its tasks. The coordinator returns only
   task identity and target for the authenticated principal.
4. The surface attaches to a selected task with an optional last-seen sequence.
5. A surface submits a stable command ID and text input.
6. The coordinator derives principal and surface identity from the authenticated
   device and gets the target and node binding from the task, then persists the
   command.
7. The same transaction records the command as pending for execution.
8. The coordinator sends only the task identity and normalized command to the
   bound node. Node reconnection replays every still-pending delivery.
9. The node adapter resolves its local harness configuration, durably admits
   the stable command ID in the harness ledger, stores its RCP
   task/command/execution mapping, and only then explicitly acknowledges
   admission.
10. The coordinator deletes the pending delivery, commits, and confirms the
   acknowledgement. A lost confirmation makes the node safely acknowledge the
   same command again.
11. The selected harness executes independently of the coordinator connection.
    Each durable local ledger commit wakes an asynchronous publisher, which may
    send contiguous event batches before the execution is terminal.
12. The coordinator binds the first `execution_started` event to the admitted
    command, rejects source gaps or mutation, assigns task sequence numbers,
    and commits the events with the next expected execution sequence.
13. The coordinator acknowledges the highest durably accepted execution
    sequence.
    Lost acknowledgements can be retried with an overlapping batch without
    adding another task-journal copy.

For gap-free attachment, the coordinator subscribes a surface to live events
before reading the durable suffix. It sends the suffix through a captured high
watermark, then ignores buffered events at or below that watermark and
continues live. An event committed during attachment therefore appears in the
snapshot or the live buffer, never in neither.

## Deliberate exclusions

- No custom channel cryptography, QUIC protocol, CBOR, or Protobuf.
- No cloud workspace, filesystem replication, or executor migration.
- No automatic failover to a different node.
- No Android, GitHub, Jira, or RCP-connected Telegram UI yet.
- No direct public coordinator listener. Internet access must cross TLS
  termination and the coordinator's bounded connection boundary.
- No generic message bus, Redis, NATS, or PostgreSQL for a single-user proof.

## Known proof shortcuts

- The shared activity profile contains complete assistant messages and a small
  durable lifecycle baseline; it does not provide token streaming.
- The Pi adapter can be configured locally with one target-bound workspace in
  read or read-write mode. RCP does not grant those tools. Shell, network,
  approval, and hostile-filesystem isolation remain unproven. OpenCode Go and
  xAI provider selection also stays local; the current xAI OAuth store is a
  `0600` plaintext database rather than operating-system credential storage.
- A node process restart closes an abandoned open execution as failed.
  Checkpointed in-execution resumption is not implemented.
- Task discovery is an unpaginated snapshot with task identity and target only;
  it has no title, presence, execution status, or live directory updates.
- Static node binding has no execution generation for safe reassignment.

These boundaries are the next RCP work, not behavior to preserve as permanent
protocol design.

## What can evolve without breaking the model

- WebSocket can gain HTTP/SSE or another ordered transport.
- SQLite can be replaced behind the coordinator when measured load requires it.
- A task can later change execution bindings after workspace and context
  checkpointing exist.
- Task payloads can later be end-to-end encrypted because routing needs task,
  node, command, event, and sequence identity—not model credentials or files.
- ACP can expose Renoa to coding editors, AG-UI can shape rich frontend events,
  and MCP can expose tools. None of them owns Renoa's task continuity.
