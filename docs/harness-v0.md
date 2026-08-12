# Renoa Harness architecture v0

## Status and authority

This document defines Renoa's durable agent-harness architecture. The
standalone model-only slice exists in `renoa-harness`: it durably admits and
orders operations, persists model intent before dispatch, settles complete
responses atomically, records uncertain attempts honestly, and recovers from
process interruption using SQLite.

RCP-bound admission, tools, approvals, cancellation, compaction, and host
movement remain later slices. Sections describing them constrain those slices;
they do not claim that those behaviors exist today.

The adjacent documents have narrower authority:

- [agent-v0.md](agent-v0.md) defines the current non-durable `renoa-agent`
  SDK;
- [rcp-v0.md](rcp-v0.md) defines cross-device task continuity; and
- [kernel-v0.md](kernel-v0.md) defines the older command-scoped reference
  executor, not this harness.

If this document conflicts with RCP, `rcp-v0.md` owns the continuity boundary.
Implementation evidence may change this design only after this document and
its reasoning are updated explicitly.

## Purpose

The Renoa Harness durably owns an agent conversation and the exact position of
its active work. It can recover after a process restart without repeating a
settled model response or blindly repeating an uncertain side effect.

The harness can run in a desktop app, VPS process, or another host without RCP.
When it is later connected through `renoa-node`, RCP will make the task
discoverable and controllable from multiple surfaces.

Its central rule is:

> Save the intent, perform the external action, then save its result and exact
> next state.

## System model

```text
surface ── RCP ── node bridge ── harness ── model adapter
                                  │
                                  └─ tool host ── workspace
```

| Identity | Owner | Meaning |
| --- | --- | --- |
| Task | RCP | Cross-device address, command order, authorization, and replay |
| Session | Harness | Conversation, active operation, recovery, and context |
| Workspace | Tool host | Files, processes, tools, secrets, and side effects |
| Attachment | Surface | One temporary UI or integration connection |

The first integration maps one RCP task to one harness session and binds that
session to one local workspace. Moving from desktop to phone changes only the
attachment. It does not create another session or move the workspace.

The harness and tool host initially share one process. The existing `Tool`
boundary is sufficient; Renoa must not invent a remote-executor service before
a real deployment needs one.

## Ownership boundaries

### `renoa-agent`

`renoa-agent` owns provider-neutral messages, model and tool ports, streaming
types, and the deterministic rules that advance a model/tool loop. Its `Agent`
remains the convenient in-memory SDK.

It does not own authoritative history, durable admission, crash recovery,
compaction, approvals, workspace binding, or RCP delivery.

The harness must not call `Agent::prompt()` as one opaque durable effect: that
method crosses several external effects without persistence boundaries. The
in-memory Agent and durable harness therefore have different orchestration
state machines. They share only leaf execution semantics proven to be
identical; the first shared primitive is one model-adapter invocation through
`sample_model`.

### Renoa Harness

The harness owns:

- immutable conversation history;
- ordered operation admission and at most one active operation per session;
- the active operation's complete current state;
- context projection and later compaction;
- frozen runtime configuration for active work;
- durable token usage and user-visible output; and
- recovery after process interruption.

The harness does not own provider wire formats, credential storage, tool
implementations, sandbox mechanisms, product permission decisions, surface
state, or task authorization.

### Node policy and tool host

Node-supplied policy decides whether an exact tool invocation is allowed,
denied, or needs approval. The harness persists and coordinates that decision.
The tool host enforces it and owns files, processes, sandboxing, tool-side
credential use, and actual side effects. The node or embedding host owns secure
credential storage and provisioning; model and tool adapters consume only the
credentials they need.

Tool-host registration also supplies a recovery class for the exact invocation
before its intent is committed. The default is `NeverReplay`; the harness
records and applies the classification but does not invent it from a tool name
or arguments.

### RCP and node bridge

RCP owns devices, task authorization, stable command admission, task order,
executor binding, and cross-device replay. It does not own model context or
harness permissions.

The bridge binds a provisioned RCP task to a harness session, maps an admitted
RCP command to one harness operation, and durably projects selected harness
output into RCP execution events. It does not reimplement the loop.

RCP commands and harness entries overlap in content but have different jobs:

- the RCP command proves that user intent was accepted and ordered across
  surfaces;
- the harness user entry places that intent in model-visible history; and
- selected completed harness output is mirrored into RCP for remote replay.

## Session ownership and admission mode

One running process holds an exclusive operating-system lock for the harness
database for its entire lifetime. A second writer fails to open it. SQLite
transactions alone are insufficient because two processes could both dispatch
the same external effect between commits. No lease or distributed lock is
needed in v0. The current Unix implementation locks a sidecar derived from the
canonical database path, rejects hard-linked database files, and verifies the
open database and lock identities around every SQLite connection. This avoids
locking SQLite's own file, which conflicts with WAL mode. Renaming, deleting,
or linking these files while the harness is live is unsupported and fails
closed when detected. The parent directory is a trusted host boundary: the
lock coordinates cooperating harness processes; it cannot fence an actor that
can unlink and replace the lock itself.

Inside that process, one ordered owner mutates each loaded session. Model and
tool workers return results to that owner; they never write session state
directly.

A session will have one immutable admission mode:

- **Standalone** — local admission assigns a monotonic position; or
- **RCP-bound** — the session is pre-bound to one RCP task and accepts new user
  and control requests only through the coordinator.

The current API and schema implement standalone sessions only, through the
explicit `create_standalone_session` and `admit_standalone` entry points.
The caller chooses a stable session ID, so a lost create acknowledgement can be
retried safely. RCP-bound creation and admission do not exist yet.

Once RCP-bound, a local app is another RCP surface; it cannot bypass the
coordinator during an outage. It may retain a command in its surface outbox,
but the harness cannot execute it until RCP admits and orders it. Already
admitted work continues during link loss. Importing or binding a standalone
history is a future explicit operation, never an automatic merge.

The task-to-session binding must exist locally before the coordinator can
dispatch that task. The first `Execute` delivery must not lazily create a new
session. Redelivery finds the same binding and stable operation admission.

## Execution hierarchy

```text
Session
  └─ Operation: one admitted user command until terminal
       └─ Step: one model request and the tool batch it produced
```

An operation has immutable admission data: operation identity, stable request
identity and content, and order. Its mutable current state is one total program
counter.

Queued user input is not conversation history. Activation atomically selects
the next operation, appends its user entry, freezes its runtime profile and
safety limits, and records that a model step is needed. Thus prompt B cannot
appear in model context before prompt A settles.

Cancellation, approval resolution, and future steering are durable **control
requests**, not queued operations. Each has its own stable identity and target.
The session owner applies it to the targeted operation without placing it
behind later user commands. Operation positions count only user commands;
controls keep their own durable admission and applied status. A late control is
resolved against its target rather than consuming or blocking an operation
position. Exact control ordering remains deferred with those features.

## Command order

Standalone admission assigns a gapless local operation position in the
admission transaction.

An RCP-bound session requires a gapless coordinator-assigned command position
within the task. Current RCP `Execute` does not carry this value. Before the RCP
integration slice, command admission and `Execute` must expose it and tests must
consume it; it must not be added as an unused wire field.

The harness may durably admit position `N` before a delayed predecessor, but it
must not activate `N` until every earlier position from the binding's starting
position is present and terminal. Reversed network arrival can never reverse
conversation order.

An RCP-delivered command has already been accepted by the coordinator. The
harness therefore has no independent queue-full rejection. It either durably
admits the operation or persists a terminal execution rejection before
acknowledging local admission. Storage failure receives no acknowledgement and
is retried. Admission quotas belong before coordinator acknowledgement.

## Durable data model

The implemented model-only slice uses one SQLite backend with these logical
records:

1. **Sessions** — identity, admission and output cursors, and active operation.
2. **Operations** — immutable admission data plus one versioned, mutable total
   current state; queued work is an operation in `Queued` state.
3. **Conversation entries** — immutable, linearly ordered user and assistant
   content associated with an operation.
4. **Model-attempt records** — attempt status, known token usage, uncertainty,
   and the exact request retained only while that attempt is recoverable.
5. **Output records** — append-only user-visible facts carrying their producing
   operation, stable record IDs, and a gapless session-local sequence.

RCP admission mode and task binding, workspace/runtime binding, and tool-result
entries are added only when their implementation slices consume them.

These are responsibilities, not a permanent table count. There is no mutable
conversation head in the linear v0 history; the latest entry is derived from
its order. There is no generic key-value register system or pluggable storage
interface.

Output rows are not deleted in v0. Observers supply their last cursor; the
harness does not store one cursor per observer. A bridge stores only its own
projection and RCP acknowledgement state outside the harness core.

SQLite opens with foreign keys enabled, WAL journaling, `synchronous=FULL`, and
a bounded busy timeout. Acknowledgements and external effects happen only
after the required transaction commits successfully. The store and operation
state carry format versions; an unknown newer version fails closed. No generic
migration framework exists before a second version needs one.

Every external request identity is bound to immutable content. An exact retry
returns the existing admission; reuse with different content is a conflict.

## Operation state machine

The current state contains everything needed to choose the next action without
replaying a mutation log or inferring meaning from a missing row.

The minimal conceptual phases are:

```text
Queued
  -> NeedModel
  -> ModelPending
  -> NeedTool -> WaitingForApproval -> ToolPending
  -> NeedModel | Completed

any active phase -> OutcomeUnknown
any active phase -> Failed | Cancelled   (only after safe settlement)
```

`WaitingForApproval` is reachable only before tool intent. It retains the exact
tool continuation. Cancellation is a durable flag alongside the current phase,
not a replacement phase that forgets the pending effect.

The state persists logical model-step count, attempt count, tool-batch index,
current tool index, and frozen limits so restart cannot reset a safety bound.
Every model attempt and tool invocation has a harness-generated effect ID and
settlement token.

A result settles only if one transaction confirms that the same operation,
phase, effect ID, and settlement token are still current. Any transition that
invalidates in-flight work rotates the token. A stale completion cannot change
conversation state; verified usage may still be recorded once under its
attempt identity.

An operation becomes terminal exactly once. Its terminal transaction commits
required tool results and output, records its outcome, clears the session's
active pointer, and may activate the next contiguous operation. RCP publication
failure never reopens completed harness work. Failure and cancellation are
operation/output facts; the harness does not fabricate an assistant answer.

## Atomic transition and effect rule

Every state transition is one SQLite transaction. No model, tool, network,
approval wait, or hook runs inside that transaction.

Every external model or tool action uses this effect sandwich:

```text
transaction: exact intent + effect ID + reserved result IDs + recovery rule
external action
transaction: complete result + usage + durable output + exact next state
```

After either transaction, reopening the database yields one valid state. A
notification channel may wake a worker after commit, but committed rows—not
the wakeup—are authoritative.

## Model steps

One opaque runtime-profile revision is frozen when an operation activates. In
the model-only slice, the supplied profile resolves the model adapter while the
harness durably freezes the system instructions and model-attempt limit.
Recovery uses those saved values even if a host incorrectly reuses a revision
with changed values. Configuration changes should still use a new revision and
apply to the next operation; recovery fails closed if the saved model binding
revision is unavailable.

Before each attempt, the harness persists:

- the exact provider-neutral `ModelRequest` sent to the adapter;
- runtime-profile revision, attempt number, and maximum attempt count; and
- effect ID, settlement token, and reserved assistant-entry and output IDs.

The request is retained only while needed for recovery; immutable conversation
entries remain the long-term history. The harness builds the request before
the shared sampling primitive, so the current arbitrary `ContextProjector`
cannot silently produce different recovery input after a restart.

One model-adapter invocation represents one inference attempt. Authentication,
token refresh, and transport continuation remain adapter concerns, but an
adapter must not secretly begin a second inference that may create another
response or charge. Observable retries are separate harness attempts.

Streaming text, reasoning, tool arguments, and progress are transient. Only a
complete response settles as an assistant entry. After a crash in
`ModelPending`, the request may have completed or incurred cost. Recovery uses
the exact saved request. If the saved policy has another attempt, recovery
marks the interrupted attempt's outcome and cost unknown, then persists a new
attempt with new effect and result identities before dispatch. If retry is
disabled or exhausted, it records the uncertain attempt and fails the
operation. Renoa does not claim exactly-once billing.

The model-only slice advertises no tools and validates the completed response
before inserting it. A provider that nevertheless returns a tool call causes a
durable failed operation; the invalid response is not inserted and cannot move
the operation into an unsupported tool phase.

## Tool execution and transcript validity

Every tool intent stores exact call, tool-binding revision, arguments, result
identity, effect ID, and recovery class.

- `SafeToReplay` means repetition is safe even if the previous invocation
  completed or is still running unobserved.
- `NeverReplay` covers everything else, including Bash, writes, deployment, and
  outbound messages unless a stronger contract is later proven.

An idempotent class may be added only when a real tool consumes a stable
invocation key and tests prove it. A missing result never implies that a tool
did not run.

Recovery may repeat a pending `SafeToReplay` call using the exact saved binding
and arguments. A pending `NeverReplay` call enters `OutcomeUnknown`; the
session performs no further model or workspace effects until an explicit
resolution. The tool slice includes one minimal `abandon unknown` control: it
appends an honest error result saying that the pending call's outcome is
unknown and it was not retried, appends cancelled-before-start results for the
remaining calls in that batch, and fails the operation. It never guesses that
the external effect failed. Richer reconciliation remains future work.

A tool emits zero or more transient progress updates followed by exactly one
final result. Durable harness v0 executes each model-produced batch
sequentially. Parallel batches remain an in-memory Agent feature until crash
tests prove independent recovery and source-ordered settlement.

Before another model request or operation activates, every settled assistant
tool call has exactly one durable result in source order, including denied,
unavailable, cancelled-before-start, and reconciled interrupted calls.

The harness may be temporarily structurally incomplete while paused on an
in-flight call, but it never presents that transcript to a model. A failed or
cancelled operation needs no fabricated conversation message; its existing
entries remain history and its terminal status is exposed through durable
output.

## Approval and cancellation

These are later features, but their safety boundaries are fixed now.

An approval request is persisted before any surface sees it. It has a stable
identity, and the first authorized resolution wins idempotently across devices.
Approval occurs before tool intent and does not make an unsafe tool replayable.
RCP transports the question and answer; node policy remains authoritative.

Cancellation records a durable targeted request and signals the current model
or tool. A terminal cancellation is allowed only after transcript validity is
restored and the executor confirms that no local invocation can still run. A
process tool must kill and reap its process group. If an unsafe external effect
cannot be proven stopped, the operation remains `OutcomeUnknown`; it does not
start the next operation after an arbitrary timeout.

When cancellation invalidates pending work, it rotates the saved settlement
token. A late completion carrying the older token cannot settle output.
Dropping an execution future is not ordered cancellation.

## Observation and RCP publication

The implemented model-only API exposes a diagnostic `inspect()` snapshot with
the complete conversation, operation statuses, and one terminal durable output
per finished operation. It does not yet expose cursor reads, a subscription,
or transient streaming. `inspect()` is not the future RCP bridge API.

The RCP integration slice must add:

1. an append-only durable output log whose records identify their operation and
   applicable effect and carry stable IDs, immutable content, and a replay
   cursor; and
2. live token, reasoning, tool-argument, and progress deltas that may disappear
   on process or connection failure.

Snapshot plus subscription is one gap-free operation: subscribe before reading
through a captured high-water mark, then discard buffered records at or below
that mark and continue live. Slow observers reconnect from their last applied
cursor.

Harness result and output are committed together. The bridge scans the durable
log from its own consumed cursor. It inserts the RCP projection and outbox and
advances that cursor in one transaction, then publishes until acknowledged.
Applying an RCP acknowledgement and advancing the outbox cursor are also one
transaction. Missing a wakeup or losing the link loses nothing because output
remains queryable.

The RCP integration must persist this one-to-one mapping:

```text
one RCP command -> one harness operation -> one RCP execution ID
```

Projected RCP events receive a stable event ID and a gapless
operation-relative sequence starting at zero, as RCP requires. This is distinct
from the harness's session-local output cursor. Retries reuse the exact event
identity, sequence, timestamp, and payload.

## Context and compaction

The harness owns complete immutable conversation history. Model context is a
derived projection, and the exact active request is persisted before sampling.
Initial implementation uses the full provider-compatible transcript.

Compaction remains later work. It may add an immutable checkpoint and atomically
select it for future context, but it never rewrites history or moves workspace
state. No checkpoint pointer or compaction schema exists before that feature.

## Host continuity

Surface or RCP-link loss does not cancel local execution. Process loss recovers
from the saved operation state. Machine loss is different: the task remains
visible through RCP, but execution pauses because workspace and credentials
belong to that host.

Moving execution requires a harness snapshot, workspace checkpoint, compatible
runtime and tools, provisioned credentials, and verified quiescence of the old
executor. A newer RCP execution generation fences stale protocol publication;
it cannot stop an old node's local or external side effects. Tool-level fencing
is required where quiescence cannot be proven.

Automatic host failover is forbidden in v0. A coordinator must not reassign a
task merely because its node appears offline: the old node is allowed to keep
working during a coordinator outage.

## Locked decisions

1. A session is linear, has one active operation, one in-process owner, and one
   lifetime-exclusive database writer.
2. Standalone and RCP-bound admission are distinct and do not silently merge.
3. Input is durable before acknowledgement and deduplicated by a stable request
   identity bound to content.
4. RCP-bound activation follows a coordinator-assigned gapless command order.
5. Queued user input enters conversation only when activated.
6. Every external effect has a persisted intent, unique effect ID, and
   settlement token.
7. Every transition stores complete current operation state; settlement uses
   state, effect, and settlement-token compare-and-set.
8. Result, usage, durable output, and next state are atomic when they arise from
   the same effect.
9. Partial streams are transient; complete messages and tool results are
   durable.
10. Unsafe uncertain effects pause and are never automatically replayed.
11. A new model or operation never sees a structurally incomplete transcript.
12. Runtime configuration is frozen per operation and missing revisions fail
    closed.
13. Harness history, RCP history, and workspace state remain separate.
14. The harness operates without RCP and continues admitted work during link
    loss.
15. SQLite is the only v0 backend, with full commit durability.
16. Agent and harness share proven leaf execution primitives while retaining
    orchestration appropriate to in-memory and durable state respectively.
17. Execution stays on one host until checkpointing, quiescence, and fenced
    rebinding are proven.

## Stress-test contract

Every implemented slice must prove both observable states around each commit
and external action on its real path. Explicit crash injection is required at
post-commit and post-dispatch boundaries; an ordinary unopened call or rolled
back real transaction may prove the indistinguishable pre-commit side.

### Model-only slice

- a second process cannot acquire writer ownership;
- exact admission retries deduplicate and changed content conflicts;
- a crash around activation cannot duplicate or reorder the user entry;
- crashes before intent, after intent, after dispatch, and before settlement
  reopen to one explicit state;
- retry uses a structurally identical provider-neutral `ModelRequest` and
  saved runtime revision, while a missing revision fails closed;
- stale effect IDs or settlement tokens cannot settle current work;
- restart preserves attempt, step, and safety-limit counters;
- an unexpected tool call fails closed without entering tool state; and
- required SQLite durability settings are verified on the actual connection.

### Tool slice

- every crash position around a tool intent and result is injected;
- safe calls replay with exact saved input;
- unsafe pending calls pause without starting later work;
- abandoning an unknown unsafe call writes honest, source-ordered results and
  unblocks the session without replaying the effect;
- unavailable tool revisions fail closed;
- every settled call receives exactly one source-ordered result; and
- cancelled or failed operations restore a provider-valid transcript before
  later activation.

### RCP integration slice

- deliberately reversed deliveries still activate in command order;
- a missing predecessor blocks later activation without blocking admission;
- task/session provisioning and exact redelivery resolve one operation;
- an RCP-bound local surface cannot bypass coordinator order;
- link loss does not stop admitted work;
- bridge crash and lost RCP acknowledgements produce one projected event;
- each command maps to one execution stream beginning at sequence zero; and
- output committed between snapshot and listener registration is still
  delivered exactly once in the reconstructed view.

Approval, cancellation, compaction, and executor migration gain their own race
tests when each feature is implemented; they are not prerequisites for the
model-only slice.

## Implementation order

1. **Implemented:** the standalone model-only SQLite harness exists and shares
   the one-attempt `sample_model` primitive with `renoa-agent`.
2. Extend RCP command delivery with consumed ordering data and connect the
   model-only harness end to end through the node bridge.
3. Add durable sequential tools with `SafeToReplay` and `NeverReplay` recovery.
4. Add approval and cancellation as separate complete features.
5. Add steering, compaction, and coding-tool packages only through real product
   paths and tests.
6. Design workspace checkpoints and host movement only after local continuity
   is reliable.

Every slice removes any temporary path it replaces and passes formatting,
lint, and workspace tests before the next begins.

## Explicit non-goals for v0

- branches, lanes, subagent trees, or concurrent operations in one session;
- durable parallel tool execution;
- automatic host migration or failover;
- workspace backup or filesystem synchronization;
- provider stream resumption or exactly-once billing;
- exactly-once external side effects;
- pluggable storage, JSONL, Postgres, or a memory backend;
- generic hooks, middleware, plugins, or application state cells;
- full-text search, labels, retention, or compliance deletion;
- a universal event catalog or provider-specific events in RCP;
- Temporal, a workflow engine, broker, distributed lock, or remote tool
  service; and
- compatibility with the old `renoa-runtime` ledger.

## Open decisions

- Product defaults for the model-attempt limit; the harness currently requires
  an explicit non-zero value.
- A durable registry that resolves each runtime-profile revision to one model
  adapter identity; system instructions and attempt limits are already frozen.
- The concrete RCP task-to-session provisioning identity; standalone creation
  is already retry-safe with a caller-stable session ID.
- Cursor-based output reads for the future RCP bridge.
- The future idempotent-tool recovery contract and reconciliation UI.
- Durable approval, cancellation, and steering operation shapes.
- Compaction policy and checkpoint format.
- Workspace identity, snapshots, executor quiescence, and tool fencing.
- Retention and deletion policy.

An open choice becomes locked only when code consumes it, deterministic tests
prove the invariant, and this document records the reason.

## Reference evidence

No upstream source is incorporated. The design was informed by:

- Pi `origin/dev` at `cf0102b9ce79b18094537b44d045b2504a030322`
  (MIT): immutable entries, total current operation state, and
  intent/effect/settlement recovery. Its redesign is unfinished and explicitly
  excludes replication.
- OpenAI Codex CLI `origin/main` at
  `95aada11c4150e4ba28d6279c50f0995c1d93e5a` (Apache-2.0): ordered session
  ownership, step snapshots, cancellation, tools, and reconnectable control.
  Its history recovery does not safely resume uncertain side effects.
- Grok Build `origin/main` at
  `be713136d2a69080743a3f6b3c72077057e5948f`, source revision
  `d6937fe255dce4133c3d000a50f9cb94de12f06f` (Apache-2.0): separated sampling,
  conversation, tools, workspace, and final tool results. Its current prompt
  queue and parts of relay delivery do not meet RCP durability.
- Cursor's published cloud-agent, Remote Control, and My Machines design:
  separated conversation, agent execution, and workspace state. Renoa does not
  adopt its proprietary services, Temporal, or cloud-VM infrastructure.
