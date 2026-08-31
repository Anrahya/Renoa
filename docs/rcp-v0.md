# Renoa Continuity Protocol architecture v0

## Status and authority

This document is the canonical architecture contract for the Renoa Continuity
Protocol (RCP). It records the decisions that future implementation work must
preserve.

RCP v0 is not yet a stable public wire specification. Its proven operation
semantics and candidate JSON/WebSocket binding are documented separately. The
Rust coordinator remains a plaintext loopback server, while the first public
deployment terminates TLS at `wss://renoa.live/connect`. Independent Rust and
TypeScript peers cover both roles, recovery, errors, exact-number boundaries,
a real OpenAI-compatible Pi turn, and two-surface continuation. The binding is
still a candidate until a real Agent turn and surface handoff are proven
through the public origin.

The related documents have narrower authority:

- `continuity-v0.md` describes the current coordinator and continuity proof.
- `identity-v0.md` describes the current device trust mechanism.
- `kernel-v0.md` describes one optional executor implementation.
- `rcp-operations-v0.md` defines the proven transport-independent operations.
- `rcp-json-ws-v0.md` defines the candidate version 8 JSON/WebSocket binding.

If one of those implementation documents conflicts with this architecture, this
document owns the intended RCP direction and the conflict must be resolved
explicitly.

## Definition

RCP is a protocol for authenticated surfaces and execution nodes to discover,
control, execute, observe, disconnect from, and resume durable agent tasks.

Its central rule is:

> Connections are temporary. The task is not.

The task is not a session inside an agent process. It exists independently of
the process, model provider, agent harness, surface, and network connection that
happen to be active at a given moment.

An agent harness is a replaceable executor of task commands. Renoa's Rust
kernel, Pi SDK, or another harness can fill that role without becoming part of
RCP.

## Product outcome

A person pairs each device with Renoa once. When they open any authorized
surface, they can discover their tasks, reconstruct each task from its last
saved cursor, submit the next command, and observe work performed on the bound
execution environment.

Changing surfaces does not transfer a process, credential, or private local
state. The new surface authenticates as its own enrolled device and attaches to
the same durable task. A device with no cursor requests the full journal; a
returning device resumes from the cursor it applied locally. The bound node
keeps the model, tools, workspace, and their credentials throughout that
surface handoff.

For example:

```text
Mac app ──────────┐
Android app ──────┤
Telegram adapter ─┼── task link ── coordinator ── executor link ── node
GitHub adapter ───┘                       │                         │
                                         │                         └─ Pi SDK,
                                  durable task journal                Renoa kernel,
                                                                      or another harness
```

Interaction continuity and environment continuity are separate outcomes. RCP
keeps a task addressable and its communication recoverable. Moving a filesystem
or live process to another machine requires a separate workspace and checkpoint
system.

## Locked decisions

These decisions define RCP and are not ordinary implementation details:

1. The durable task is the center of the system.
2. A connection never owns task identity or authoritative task state.
3. The coordinator owns task admission, authorization, ordering, and replay. It
   does not run an agent loop, call a model, or execute tools.
4. Surfaces capture input and project task state. They do not decide execution
   authority.
5. Nodes own execution environments, local side effects, model credentials, and
   harness state.
6. Agent harnesses are replaceable adapters behind an execution node.
7. Every durable task record has one coordinator-assigned total order within its
   task. Timestamps never determine that order.
8. Important delivery is at least once. Stable producer identities and durable
   deduplication make retries safe.
9. An acknowledgement of admission means the acknowledged data is durable. It
   does not mean execution has started or completed.
10. Any user-visible fact required after reconnection must be represented by
    durable task state. Presence and transport health may remain ephemeral.
11. Authentication identifies a device; authorization is still checked for
    every task operation.
12. RCP semantics are independent of transport encoding, storage engine, cloud
    vendor, model provider, surface, and agent harness.
13. RCP does not invent channel cryptography or claim end-to-end exactly-once
    side effects.
14. RCP authorizes access to tasks and which node may execute them. It does not
    define or enforce what a harness may do inside its environment.
15. An execution delivery contains continuity data only. System instructions,
    models, context policy, tools, capability policy, and provider credentials
    remain behind the node's harness adapter.
16. Every durable execution task record names the stable command that caused
    it. Surfaces must not infer execution causation from arrival order,
    adjacency, or timestamps.
17. Every installation has its own revocable device credential. Device
    credentials are never copied or transferred to continue a task on another
    surface.
18. Surface continuity transfers authority to observe and submit to the same
    task; it does not transfer the executor process, harness context, workspace,
    provider credentials, or tool credentials.
19. Surface cursors and command outboxes are device-local recovery state. A new
    surface can reconstruct the task from the coordinator journal without
    receiving another surface's local database.
20. Renoa's first self-hosted deployment is a control plane with one
    logical coordinator authority per task, reached at a stable HTTPS origin.
    Nodes and surfaces initiate connections to it. The origin and route are
    deployment configuration, not RCP task semantics.

## Vocabulary

### Principal

The person or organization that owns tasks and authorizes devices.

### Device

One enrolled installation with a stable, revocable identity. A device's role is
selected by the coordinator during enrollment and is never self-declared on a
later connection.

### Surface

A user-facing or integration-facing participant that submits commands and
projects task records. Native Renoa applications may speak RCP directly.
Telegram, GitHub, Jira, and similar systems use adapters because those systems
do not speak RCP themselves.

### Task

A durable address, an authorization boundary, an ordered journal, and an
execution binding. A task can contain many commands and executions. It remains
addressable after an individual execution reaches a terminal state.

### Command

A durable intent submitted by a surface. Its producer assigns a stable identity
before transmission so an uncertain delivery can be retried without creating a
second intent.

### Execution

One attempt by an authorized node and harness to process a command. Command
admission and execution are distinct facts.

### Task record

One immutable entry in a task journal. The coordinator assigns its task
sequence only after durable admission. A task record carries a stable identity,
its task identity, its task sequence, causation information where required, and
a versioned payload. The current binding is documented; the permanent
cross-harness record envelope remains deliberately unsettled.

### Cursor

The last contiguous task sequence durably applied by a consumer. A cursor is a
position in authoritative task history, not a connection identifier.

### Node

An enrolled execution environment such as a Mac, VPS, or future sandbox. Nodes
open outbound connections, accept authorized work, durably admit it locally,
and publish execution events.

### Execution authority

The coordinator's permission for one node to execute work for a task. The
current proof uses a static node binding. Rebinding or failover will require an
increasing generation that rejects output from older executors after authority
moves.

### Coordinator

The logical authority that authenticates peers, authorizes task access, admits
commands and execution events, assigns task order, stores the journal, tracks
execution authority, and serves replay. It may be implemented by one Rust
process, a per-account or per-task actor, or another storage-backed service.

## Protocol layers

RCP is split into three layers so agent evolution does not force continuity
redesigns.

### Semantic core

The core defines:

- device authentication and role binding;
- task discovery and authorization;
- command admission and deduplication;
- task record ordering and replay;
- execution availability, dispatch, and acknowledgement;
- execution event admission and deduplication;
- cancellation and failure semantics when implemented;
- version and error behavior.

The semantic core does not define model requests, context construction, tools,
system prompts, token accounting, provider authentication, or harness
permissions.

### Activity profiles

An activity profile defines how a particular class of executor output is
represented to surfaces. Profiles can cover text streaming, tool activity,
approvals, diffs, artifacts, or harness-specific events.

RCP does not standardize one universal agent-event model. Rust and Pi now share
one deliberately small durable baseline: execution start, turn start, complete
assistant text, tool start, tool result, and terminal outcome. Provider events,
token deltas, reasoning blocks, usage, and harness message schemas remain local
unless a later consumer proves they belong in continuity.

AG-UI may provide a general frontend profile, ACP may provide a coding-surface
adapter, A2A may provide an agent-to-agent gateway, and MCP may expose tools.
Those protocols do not own RCP task authority, durable replay, or executor
binding.

### Transport bindings

A transport binding maps RCP operations and records onto a concrete channel.
The first binding is inspectable JSON over WebSocket. HTTP commands, SSE,
webhooks, or another ordered transport may be added without changing task
semantics.

RCP v0 does not require custom QUIC, WebTransport, Protobuf, CBOR, a generic
message broker, or application-layer encryption.

## Authoritative task journal

Each task has one append-only authoritative journal.

The coordinator serializes admission for a task and assigns monotonically
increasing sequences without gaps among committed records. Concurrent surfaces
may submit commands, but commit order is the authoritative order.

A projection, snapshot, cache, conversation transcript, or LLM context can be
derived from the journal. None of those derived views replaces the journal as
authority. Context construction remains a harness concern and may intentionally
omit, compact, or transform task history.

The journal stores semantic facts, not connection mechanics. Heartbeats,
temporary socket identifiers, typing indicators, and transient presence do not
belong in it unless product behavior later requires them after reconnection.

Large files and workspace contents will eventually live outside the journal and
be referenced as artifacts. Blob storage, retention, compaction, and snapshot
formats remain open because the current runtime does not consume them.

## Surface link

### Discovery

An authenticated surface must be able to discover tasks authorized for its
principal. Pairing a device with the coordinator is sufficient; users must not
pair the same phone separately with every task or node.

The current proof implements an initial `ListTasks` snapshot. It returns only
task identity and target, filters by the authenticated principal, and orders by
task identity. Pagination, user-facing metadata, presence, and live directory
updates remain open until a real surface requires them.

### Attachment and replay

A surface attaches with a task identity and its last contiguous cursor. The
coordinator returns every committed record after that cursor and then continues
with live records without a gap between replay and live delivery.

The surface applies records only in task-sequence order. Duplicate record
delivery is harmless. A missing sequence forces replay instead of guessing.

A slow surface cannot block task execution. The coordinator may disconnect it;
the surface recovers from its durable cursor and the journal.

### Command submission

Before sending, a surface durably retains its command identity and payload. The
coordinator atomically checks authorization, deduplicates the command identity,
commits the command, and only then acknowledges admission.

If the connection fails before acknowledgement, the surface retries the same
identity. Reuse of an existing identity with different content is a conflict.

An admitted command is not automatically a running command. The observable
execution facts must remain distinct:

```text
not admitted -> admitted -> dispatched -> running -> terminal
```

The current proof stores only the fact required for reliable delivery: whether
an admitted command is still pending node admission. A richer dispatched,
running, and terminal state model remains deferred until a surface or executor
feature consumes it.

## Executor link

### Availability and dispatch

A node authenticates and the coordinator derives its node identity from the
enrolled device. A task may dispatch only to its authorized node or future
execution authority.

Reliable dispatch cannot depend on an in-memory socket send. Once a command is
admitted for execution, the coordinator retains a durable pending delivery
until an authorized node durably admits and acknowledges it. Reconnection must
redeliver unacknowledged work with the same identity.

The Rust proof inserts the pending delivery in the same SQLite transaction as
the command and its task event. A current, task-bound node sends
`acknowledge_execution` only after local durable admission. The coordinator
deletes the pending record, commits that deletion, and then sends
`execution_acknowledged`. Both messages are idempotent by command identity.

Node activation takes a pending-work snapshot before the new connection becomes
eligible for live dispatch. Command admission is serialized with that short
activation boundary, preventing an in-flight command from landing between the
snapshot and connection replacement. Model execution and socket delivery stay
outside the lock.

The node's local harness ledger is responsible for deduplicating that execution
identity before model inference or side effects begin.

The reference `renoa-node` bridge stores the RCP task, command, and run mapping
in the same local SQLite ledger as the kernel run. It acknowledges execution
only after both the kernel admission and that mapping are durable. A process
crash between those commits leaves the coordinator delivery pending, so the
same command is redelivered and the mapping can be reconstructed safely.

Socket ownership stays outside the Engine. Durable ledger commits wake the
bridge publisher, but the wakeup is not data: the publisher always reads the
ledger from its last coordinator-acknowledged source cursor. Model and
capability execution therefore continue across coordinator-link loss. After
reconnection, an uncertain event batch is resent from the durable cursor.

Renoa does not yet checkpoint a running agent loop. If the node process itself
restarts with a mapped run still open, the bridge records a failed terminal
event instead of repeating the command and risking duplicate side effects.
True in-run resumption remains separate future work.

### Harness adapter boundary

An `Execute` delivery contains the task identity and normalized command. It
does not contain an agent identifier, system instructions, model selection,
tool declarations, or capability grants. The durable task identity and target
let the receiving node select and validate its local harness binding without
making that binding part of RCP.

A harness adapter has four continuity responsibilities: durably admit a stable
command before acknowledging it, invoke its locally configured harness,
project supported durable activity, and resume publication from its last
acknowledged source cursor. It does not reimplement the harness's agent loop,
context policy, provider integration, or permission system.

The Rust node resolves its `ResolvedAgent` locally. The Pi node resolves its
provider credentials, instructions, model, target, and optional workspace tools
locally. Both consume the same RCP delivery and publish the same baseline
activity without a harness-specific branch in the coordinator. Adding another
harness therefore requires a small adapter at the node boundary, not an RCP
architecture change.

### Execution event publication

An executor persists important events locally before publication. It sends
stable event identities and source ordering in replayable batches. The
coordinator validates authority, deduplicates the events, assigns task
sequences, commits them, and then acknowledges the accepted source position.

An acknowledgement lost in transit causes the node to resend the same events.
The task journal still contains one copy.

The source execution order and coordinator task order are separate. The former
proves a contiguous execution transcript; the latter gives every surface one
authoritative cross-command view.

The Rust proof now persists one source cursor per command-bound execution. A
batch may start at or before the next expected execution sequence, allowing
exact overlap after an acknowledgement is lost. Previously accepted events
must retain the same identity and content. New events must begin exactly at the
cursor; gaps, a different execution identity, and events after
`execution_terminated` are rejected. The acknowledgement reports the highest
execution sequence committed with the task journal.

### Authority changes

Only one execution authority may publish for a task at a time. Before Renoa
supports rebinding, the authority must gain an increasing generation or
equivalent fencing value. After generation 8 is active, a node holding
generation 7 cannot publish, even if an old connection wakes up.

The exact lease and generation messages are deferred until task rebinding has a
real execution path and test.

## Offline behavior

RCP defines delivery facts; product policy decides whether a new command should
be admitted while no executor is available.

A deployment may reject it before admission, admit it as visibly pending,
expire it, or require confirmation before delayed execution. Whichever policy
is selected must be explicit. An acknowledged command cannot silently vanish,
and an unacknowledged command must be safe to retry.

The loopback proof currently rejects new work while the bound node is offline.
That behavior is not yet a permanent RCP default.

Connection loss never implies that a task, command, or execution completed.

## Delivery guarantees

RCP uses four persistence rules:

1. A producer persists important outbound data before sending it.
2. The coordinator persists admitted data before acknowledging it.
3. A consumer persists its last contiguous cursor only after applying the
   corresponding records.
4. A producer removes outbound data only after receiving an acknowledgement
   that identifies the durable admission point.

Network delivery is at least once. Durable insertion is idempotent by stable
identity. Renoa does not claim end-to-end exactly-once execution because a
process can fail after an external side effect succeeds but before its outcome
is durably known.

Capabilities that perform side effects need their own idempotency or
reconciliation contracts before automatic crash recovery can safely repeat
them.

## Security boundary

RCP v0 uses the enrollment and per-device credential design in
`identity-v0.md`:

- the coordinator selects principal and role before enrollment;
- plaintext credentials are not stored by the coordinator;
- revocation blocks future connections and terminates active ones;
- task ownership is checked for attachment and command admission;
- only the authorized node may publish execution events.
- only the authorized node may acknowledge execution admission.

Production transport requires TLS. Credentials are never placed in URLs.
Bearer device credentials are sufficient for the personal-runtime proof but
must be stored in operating-system credential storage.

Sender-constrained credentials, account recovery, credential rotation,
end-to-end encrypted task payloads, and multi-user collaboration require their
own threat models. RCP will reuse established security standards rather than
invent cryptography.

The coordinator may see plaintext task content in the initial architecture.
End-to-end encryption remains possible because routing fundamentally needs task,
record, sequence, device, and node metadata rather than model credentials or
workspace files.

### Credential domains and handoff

Renoa has separate credential domains which must not collapse into one shared
secret store:

- An RCP device credential authenticates one installed surface or node. The
  coordinator stores only its digest; the device stores the plaintext in its
  platform credential facility. Desktop and mobile clients use a keychain or
  keystore; a headless service uses its service manager's credential facility,
  with an owner-only file as an explicit fallback. Losing a device is handled
  by revoking it and enrolling a replacement, not by copying its credential.
- A browser surface must not put a long-lived device credential in JavaScript
  storage. The production browser binding will authenticate the person at the
  coordinator's HTTPS origin and exchange that authenticated session for a
  short-lived, single-use connection ticket. The current JSON/WebSocket bearer
  frame remains a proof binding, not that browser storage design.
- A surface's cursor and command outbox are continuity records, not
  credentials. They contain enough stable identity to replay or retry but grant
  no authority by themselves.
- Provider, MCP, and service credentials belong to the execution node. A phone
  continuing a Linux-bound task never receives them. Portable agent profiles
  may name credential references, but each authorized node resolves those
  references against its own secret facility.

A future shared-secret facility belongs to Host management, not RCP. It may be
co-located with the self-hosted control plane, but it has a separate store and
authorization contract. It can provision an explicitly authorized execution
node or broker use of a secret; attaching another surface must never trigger
secret release. The concrete encryption, recovery, OAuth-refresh ownership,
and node-grant design remains open until a second Host consumes it.

Moving execution to another node is therefore a different operation from
moving between surfaces. It requires a fenced execution-authority change,
workspace and harness recovery, and explicit proof that the destination can
resolve every required credential. RCP must never smuggle secrets through task
records to make migration appear successful.

## Self-hosted deployment and transport independence

The first internet-reachable topology is a self-hosted, always-on control plane
reachable at one stable HTTPS origin. The deployment terminates public TLS;
whether a reverse proxy or the coordinator owns termination is not an RCP
semantic. One logical coordinator authority owns identity, authorization, task
ordering, routing, and replay for each task; it does not become a model or tool
execution host. Linux, macOS, VPS, mobile, and browser participants all initiate
outbound HTTPS or WebSocket connections, so an execution node does not expose
an inbound agent port.

After enrollment, a device remembers that origin. Tailscale may carry the
connection during private development, and a normal public route or optional
relay may carry it later. No RCP frame names Tailscale, a DNS provider, a
reverse proxy, or a relay.

The logical requirement remains one serialization authority per task, not one
specific cloud product.

The reference implementation remains a Rust coordinator with SQLite because it
is small, self-hostable, inspectable, and already proves transactional ordering.
A hosted implementation may later use a durable actor such as Cloudflare
Durable Objects. Larger deployments may shard tasks or replace storage behind
the same semantics.

Nodes initiate outbound connections. Renoa does not require users to expose
their laptop with an inbound port, SSH tunnel, or remote-desktop session.

## Relationship to other protocols

- [AG-UI](https://docs.ag-ui.com/) standardizes agent-to-frontend events. It can
  inform an RCP activity profile.
- [ACP](https://agentclientprotocol.com/) standardizes coding client-to-agent
  interaction. It can expose an RCP task to compatible editors.
- [A2A](https://a2a-protocol.org/latest/) standardizes independent agent
  collaboration and task exchange. It can act as a gateway to an RCP task.
- [MCP](https://modelcontextprotocol.io/) standardizes access to tools,
  resources, and prompts. It belongs behind an executor, not in RCP continuity.

RCP does not fork or replace those protocols. Its distinct responsibility is a
complete authorized task journal plus execution binding that remains stable
when surfaces, sockets, nodes, and agent harnesses change.

## Current implementation

The current implementation demonstrates:

- server-selected device identity and revocation;
- task ownership checks;
- owner-filtered task discovery with deterministic task ordering;
- stable command and event identities;
- stable command causation on every durable execution task record;
- atomic command admission;
- one coordinator-assigned task sequence;
- gap-free replay followed by live delivery;
- two surfaces observing the same kernel-backed execution;
- static routing to one authenticated node;
- a durable pending-execution outbox committed with command admission;
- explicit, idempotent node admission acknowledgement;
- redelivery after node reconnect and coordinator restart;
- bound-node authorization for execution acknowledgement;
- race-safe command admission during concurrent node replacement;
- incremental execution-event batches with a durable per-execution source cursor;
- idempotent overlap after a lost event acknowledgement;
- source-gap, mutation, execution-identity, and post-terminal validation;
- a live `renoa-node` bridge that publishes committed kernel events before the
  model call returns;
- node transport reconnection without interrupting the running Engine;
- conservative process-crash recovery that terminates, rather than repeats,
  an abandoned open run;
- transport-independent authenticated operation dispatch beneath the first
  JSON/WebSocket binding;
- a documented version 8 JSON/WebSocket shape with binding-level conformance
  assertions;
- exact admitted-command retries succeeding after the bound node goes offline;
- rejection of replay cursors ahead of durable task history;
- enrolled-device authentication across coordinator restart;
- SQLite-backed task and identity state using WAL with full commit durability;
- a headless TypeScript surface that authenticates, discovers tasks, replays and
  follows their journals, exposes disconnect reasons, and reconnects existing
  attachments;
- a surface-side SQLite cursor committed only after event projection succeeds;
- a surface-side SQLite command outbox that survives process restart and safely
  recovers from a deliberately lost admission acknowledgement or retains work
  rejected while its node is offline;
- typed authentication, authorization, and replay-required failures across the
  Rust-to-TypeScript boundary;
- deterministic slow-surface recovery through the coordinator's real live
  buffer;
- rejection of inbound JSON integers outside the JavaScript-safe range and
  suppression of any non-interoperable outbound frame, with the same exactness
  check in the TypeScript decoder;
- a harness-neutral `renoa-protocol` crate used by the coordinator without a
  dependency on Renoa's Rust kernel;
- coordinator migrations that preserve old Rust run events and remove
  harness configuration from durable RCP commands without changing command
  identity;
- a TypeScript Pi node that durably admits work before acknowledgement, keeps
  queued work and Pi conversation context in SQLite, publishes through stable
  source cursors, reconnects independently of model execution, and fails an
  interrupted execution rather than repeating it;
- one harness-neutral `Execute` shape consumed by both the Rust and Pi nodes,
  with each node resolving its agent configuration locally;
- a real Pi SDK read-and-edit turn through Pi's OpenAI Completions adapter, a
  local compatible endpoint, the Rust coordinator, and an attached surface,
  including a real file mutation and recovery from a lost execution-event
  acknowledgement;
- local Pi provider selection for OpenCode Go and xAI, plus a headless
  SuperGrok device login whose renewable credential is durably rotated without
  adding provider data to RCP. A manual live proof exercised Grok's real model,
  Pi's edit tool, and token refresh; deterministic tests retain the external
  model boundary;
- a private VPS coordinator reached through Tailscale Serve, with trusted local
  node and task provisioning. A Mac Pi node completed a real SuperGrok edit
  while its TypeScript surface was disconnected; reconnect replayed the missed
  ordered suffix, including exactly one completed terminal event.
- two independently enrolled surfaces continuing one kernel-backed task without
  sharing credentials or cursor state: the second surface reconstructs the
  first turn, submits the next turn, and the first surface later replays that
  exact suffix with its original credential.
- a public Cloudflare Tunnel route at `wss://renoa.live/connect` while the
  coordinator remains bound to loopback. A disposable surface enrolled,
  authenticated with binding version 8, and completed `list_tasks` through the
  public origin. The full public Agent-turn handoff remains unproven.

The proof deliberately does not yet satisfy the full RCP architecture:

1. Execution binding is static and has no generation for safe reassignment.
2. The coordinator listener is plaintext and loopback-only. Public WSS is
   currently supplied by an outbound Cloudflare Tunnel, so the protocol does
   not depend on the tunnel provider and no public origin port is exposed.
3. The Pi adapter currently has one process-local harness configuration and an
   optional workspace binding with read or read-write tools. A durable
   multi-task harness registry, shell, network, approvals, and
   hostile-filesystem isolation remain unproven. Its model credential database
   is owner-only plaintext rather than operating-system credential storage.
4. The shared activity profile carries complete durable events, not transient
   token deltas or a general streaming UI protocol.
5. Person authentication, browser connection tickets, first-device bootstrap,
   recovery, and credential rotation are designed boundaries but are not
   implemented.
6. Public resource limits cover total connections, authentication time, and
   message size. Per-source throttling, health monitoring, backup restoration,
   and operational alerting remain deployment work.

These are known proof boundaries, not hidden guarantees.

None of these boundaries prevents a new harness from implementing the existing
neutral execution delivery and event profile. Renoa's Rust agent can therefore
proceed independently while later RCP work is driven by a real approval,
multi-task, or public-network consumer.

## Implementation sequence

Work proceeds in this order unless evidence changes the dependency:

1. Prove one real Agent turn through the public origin, then disconnect one
   independently enrolled surface and continue the task from another.
2. Add same-device, passkey-authorized enrollment and a browser-safe connection
   ticket at the self-hosted origin. Headless-node enrollment must be approved
   by an existing trusted device; copied credentials are not an enrollment
   mechanism.
3. Add device administration, per-source throttling, health monitoring, and a
   tested backup-and-restore procedure.
4. Add a durable harness-initiated interaction flow when a real approval or
   follow-up consumer exists; RCP transports the interaction while the harness
   retains permission policy.

Workspace replication, executor migration, mobile UI, push notifications, and
additional surface adapters follow only after the continuity path is reliable.

## Required conformance scenarios

RCP v0 is not proven until deterministic tests cover at least:

1. Two surfaces reconstruct the same task view.
2. A surface disconnects after sending but before receiving admission; retrying
   the same command creates one journal record and returns the existing
   admission even if the node has since gone offline.
3. The coordinator commits a command and loses the node connection before
   durable admission is acknowledged; reconnecting the node receives the
   pending command.
4. A node disconnects after publishing but before receiving acknowledgement;
   retrying produces one copy of every task record.
5. A surface falls behind the live buffer and recovers from its last cursor;
   a cursor ahead of durable history is rejected.
6. Coordinator restart preserves task ownership, command deduplication, pending
   dispatch, journal order, and replay.
7. Unauthorized surfaces cannot discover, attach to, or submit to another
   principal's task.
8. An unauthorized or stale node cannot acknowledge admission or publish task
   output.
9. Replacing the reference executor with Pi does not require changes to task
   admission, ordering, replay, identity, or surface behavior.
10. Coordinator-link loss does not interrupt an active local Engine run; the
    node reconnects and resumes publication from durable state.
11. A node process crash does not silently rerun an admitted open command. The
    interrupted run becomes visibly terminal until resumable checkpoints exist.
12. An execution delivery contains no harness configuration, while both the
    Rust and Pi adapters still apply their different local configurations.
13. A real Pi tool turn reads and edits inside its locally configured workspace,
    rejects parent traversal and escaping symlinks, and survives an uncertain
    execution-event acknowledgement without duplicating task history.
14. One enrolled surface can disappear after completing a turn, a separately
    enrolled surface with no shared local state can replay and continue the
    task, and the first surface can later replay the second surface's suffix.

The test harness should deliberately cut connections at persistence and
acknowledgement boundaries. Happy-path socket tests are insufficient evidence
for a continuity protocol.

## Explicit non-goals

- Designing another agent loop
- Normalizing model-provider APIs
- Defining LLM context or memory policy
- Defining tool execution or MCP semantics
- Defining or enforcing harness permissions
- Remote desktop or process tunneling
- Filesystem synchronization or workspace backup
- Automatic executor migration before checkpointing exists
- Peer-to-peer or federated task consensus
- Custom cryptography or a new network transport
- Expanding the shared activity baseline without a proven consumer
- Exactly-once external side effects

## Open decisions

These questions are intentionally unresolved and must not be filled in from
assumption after context compaction:

- The stable task-record wire envelope and versioning rules
- Node-side task-to-harness provisioning, configuration revisions, and a
  durable registry for one node hosting multiple harness configurations
- The product default for commands submitted while a node is offline
- Coordinator command-position delivery for a harness that serializes many
  admitted commands into one session
- Execution generations and safe rebinding messages
- Task-list pagination and live directory updates
- Cancellation, steering, approval, and queued-follow-up semantics
- Snapshot, retention, compaction, artifact, and blob behavior
- HTTP/SSE and webhook transport bindings
- Sender-constrained device authentication
- Passkey ceremony details, browser session lifetime, and the concrete
  single-use connection-ticket binding
- End-to-end encryption and key distribution
- Stable-origin discovery and automated TLS for a self-hosted installation
- Cross-node secret provisioning and credential-reference grants; secret
  material remains outside RCP task continuity
- Workspace checkpoint and executor migration design

An open decision becomes locked only when a real execution path consumes it, a
test proves its invariant, and this document is updated with the reasoning.
