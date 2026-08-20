# Renoa Kernel architecture v0

## Status and authority

This document defines the local durable kernel that every Renoa agent runtime
will use. It is the implementation contract for `renoa-kernel`.

[`rcp-v0.md`](rcp-v0.md) remains authoritative for cross-device admission,
authorization, routing, and replay. [`harness-v0.md`](harness-v0.md) records
the current model-and-tool harness and is implementation evidence, not the
kernel API. [`kernel-v0.md`](kernel-v0.md) describes the older command-scoped
RCP executor and does not define this kernel.

The foundation slice implements one local SQLite kernel, a decision-only loop
plugin boundary, generic effect adapters, durable cancellation, explicit
abandonment of unknown effects, durable inspection, and cursor replay. The
kernel crate intentionally does not integrate RCP, ACP, T3 Code, providers,
tools, prompts, context policy, or workspaces. Its first external consumer,
documented in
[`renoa-agent-loop-v0.md`](renoa-agent-loop-v0.md), now proves a
provider-neutral model/tool turn, honest unknown-effect recovery, and a real
local workspace edit without moving those concerns into the kernel.

## Purpose

Renoa needs one small, non-replaceable layer that preserves truth while agent
implementations vary without bound. The kernel owns identities, durable work
admission, ordering, effect safety, recovery, and observation. An agent kind is
not a kernel enum or branch. It is a runtime assembled outside the kernel from
a loop plugin, effect bindings, and configuration.

The central invariant is:

> No external action starts until its exact intent is durably recorded, and
> durable execution does not advance past that action until its outcome is
> settled or explicitly unknown.

If a coding agent, research agent, background operator, evaluator, or future
agent can be introduced by supplying a runtime without changing kernel control
flow or schema, the boundary is working.

## Ownership boundary

The kernel owns:

- stable agent, session, command, operation, effect, and event identities;
- retry-safe agent and session creation;
- exact command admission before acknowledgement;
- gapless command positions and one active operation per session;
- one in-process session driver and one lifetime-exclusive database writer;
- the frozen runtime manifest for an active operation;
- the versioned loop checkpoint and operational program counter;
- exact effect intent, dispatch state, recovery class, and settlement;
- exact-operation cancellation admission and process-local signalling;
- gapless semantic event order and cursor replay;
- atomic state transitions and fail-closed recovery; and
- database, stored-state, manifest, and checkpoint compatibility checks.

The kernel does not own:

- model or agent-loop policy;
- prompts, skills, context projection, compaction, memory, or budgets;
- provider protocols, credentials, retries, or billing policy;
- tool definitions, permissions, sandboxing, workspaces, or product policy;
- UI state, ACP, RCP, network transport, or host placement; or
- an `AgentType` catalog or agent-specific schema branches.

## Composition model

```text
host / ACP / RCP node
          |
          v
   assembled Runtime
   +----------------------------+
   | LoopPlugin                 |
   | named EffectAdapter bindings|
   | exact RuntimeManifest      |
   +----------------------------+
          |
          v
   +----------------------------+
   | renoa-kernel               |
   | identities + SQLite truth  |
   | ordering + effect broker   |
   | recovery + replay          |
   +----------------------------+
```

`LoopPlugin` is ordinary trusted Rust code. The kernel gives it an owned,
read-only snapshot containing the exact command, durable semantic history,
current checkpoint, frozen runtime manifest, and any newly settled effect. It
has no kernel handle, SQLite connection, mutable store, model, tool, filesystem,
or network access. It returns one decision:

- `InvokeEffect` with the next checkpoint, named binding, exact JSON request,
  and `SafeToReplay` or `NeverReplay` recovery;
- `AppendEventsAndContinue` with the next checkpoint and semantic events;
- `WaitForInput` with the final checkpoint and semantic events;
- `Complete` with the final checkpoint and semantic events; or
- `Fail` with the final checkpoint, semantic events, and a reason.

The kernel validates the decision, then commits it. A loop error commits
nothing and can be retried with a repaired implementation of the same exact
runtime. `WaitForInput`, `Complete`, and `Fail` terminate the current operation
and release the session to its next admitted command. Their different outcomes
remain visible to inspection.

`LoopPlugin::abandon_unknown_effect` is a separate decision-only boundary. It
receives the exact unknown effect plus the same durable command, history, and
checkpoint facts, and may return only a checkpoint and semantic events. Its
output type cannot request another effect. The host decides whether to invoke
this action; the kernel never abandons uncertainty automatically.

`LoopPlugin::cancel_operation` is the corresponding decision-only boundary for
a durable user cancellation. It receives the exact command, gapless history,
checkpoint, and any current effect fact: settled, definitely not dispatched, or
outcome unknown. It can close loop-owned history but cannot request another
effect or change the recorded external fact.

An `EffectAdapter` receives a stable effect ID, exact binding and revision, the
frozen runtime manifest, the exact saved request, and an attempt-scoped
cancellation signal. It reports either one definite success or failure outcome,
or that the external outcome is unknowable. Provider transport retries, tool
execution, and other effect-specific behavior remain inside or behind the
adapter. Process interruption is handled by the kernel recovery class.

An adapter must resolve only after work it started has stopped when the signal
is cancelled. Dropping `Kernel::drive` cancels an in-flight invocation, but a
supervised task retains both the session lease and the database writer lease
until the adapter confirms cleanup. This is process-lifecycle safety, not the
durable user-cancellation request itself.

## Durable domain

### Agent instance

An `AgentId` names one durable agent instance. It carries no agent-kind field.
Agent behavior is supplied by the runtime used to drive each operation.

### Session

A `SessionId` belongs to exactly one agent. A session owns gapless command and
event cursors plus at most one active operation. Sessions do not share history,
checkpoints, effects, or event sequences.

Commands, operations, and semantic events carry the same session ownership
through composite database constraints. This is a storage-integrity rule, not
a substitute for a future context-sharing contract.

### Command and operation

A command has a caller-stable `CommandId` and exact JSON content. Submitting the
same identity, session, and content returns the original admission. Reusing the
identity with different content or another session is a conflict.

Admission atomically creates one operation at the session's next gapless
position. Queued content is not semantic history. Activation always chooses the
lowest queued position, freezes the supplied runtime manifest, initializes the
operation program counter, and installs the session's active pointer in one
transaction.

### Cancellation

`Kernel::request_cancellation` accepts a caller-stable `CancellationId` plus one
exact session and active operation. It commits that identity and target before
signalling a process-local driver. Repeating the same identity and target is a
no-op, including after terminal settlement; reusing the identity for another
target is a conflict. A queued or terminal operation is not cancellable.

Cancellation and ordinary progress serialize through SQLite. If cancellation
commits first, no later model or tool effect may be created or dispatched. If
dispatch commits first, the kernel signals the operation-owned cancellation
token and waits for the adapter to stop its work before settlement. A definite
result remains definite; an unprovable result remains `OutcomeUnknown`.

The loop then writes any provider-neutral repair events and a final checkpoint
in the same transaction that marks the operation `Cancelled` and releases the
session. Model work has no invented assistant response. Tool work receives
call-matched results: an already-settled result is preserved, work never
dispatched is reported as not run, and possibly dispatched work is reported as
possibly completed. Later sequential calls are reported as not run. These
events are available to a future model turn without forcing another model call
inside the cancelled operation.

### Runtime manifest and checkpoint

The frozen `RuntimeManifest` contains only compatibility facts consumed during
execution:

- loop binding name and revision;
- checkpoint schema version;
- every named effect binding and its revision; and
- a host-computed configuration digest.

Recovery requires an exactly equal manifest and resolves effect decisions only
through those frozen bindings. Changing behavior that matters to recovery
requires a new binding revision or configuration digest.

A checkpoint is opaque JSON plus its schema version. Every loop decision
supplies the next checkpoint, and its version must equal the frozen manifest.
The kernel persists it but never interprets agent-specific state.

### Effects

An effect records its operation-relative ordinal, stable `EffectId`, exact
binding and binding revision, exact JSON request, recovery class, status,
dispatch count, and eventual outcome.

The v0 statuses are:

```text
IntentCommitted -> DispatchStarted -> Settled
                                  \-> OutcomeUnknown
```

`IntentCommitted` proves that adapter invocation has not started.
`DispatchStarted` proves only that invocation may have started. Settlement
atomically stores the outcome and returns the operation to `NeedDecision` with
that exact result available to the loop.

If a live adapter cannot prove a definite result, the kernel atomically marks
both the effect and operation `OutcomeUnknown`; it never launders uncertainty
into an ordinary failure. If the driving caller disappears, the adapter first
finishes cancellation cleanup, then the next drive applies the same persisted
recovery rules used after process loss.

After restart:

- `IntentCommitted` is dispatched once regardless of recovery class;
- `DispatchStarted + SafeToReplay` is dispatched again with the same effect ID
  and request;
- `DispatchStarted + NeverReplay` becomes `OutcomeUnknown` without invoking the
  adapter; and
- `Settled` is never dispatched again.

There is no exactly-once external-effect claim. Stable identity plus an adapter
that consumes it may provide stronger idempotence, but the kernel does not
invent that guarantee.

### Semantic events and execution journal

Semantic events are append-only portable facts produced by the loop. Each has
a stable event ID, operation ID, session-local gapless sequence, string kind,
and exact JSON payload. The kernel does not prescribe an agent event catalog.

The execution journal is separate: operation phase, checkpoint, current input
effect, effects, dispatch counts, and terminal outcome. Operational recovery
details do not masquerade as portable semantic history.

An `EventCursor` is the next unread sequence, initially zero. Reading after a
cursor returns all events whose sequence is at least that cursor plus the new
high-water cursor. A cursor greater than the session high-water mark is
rejected; an equal cursor returns an empty page.

## Operation state machine

```text
Queued -> NeedDecision
NeedDecision -> NeedDecision  (append and continue)
             -> EffectIntent -> EffectDispatched -> NeedDecision
                                             \-> OutcomeUnknown
             -> Waiting | Completed | Failed
NeedDecision | EffectIntent | EffectDispatched | OutcomeUnknown
             -> Cancelled  (durable cancellation)
OutcomeUnknown -> Failed  (explicit abandonment)
```

Only `Queued` operations lack a manifest. Every other phase has the exact
frozen manifest and a checkpoint after the first committed loop decision.
`NeedDecision` may carry one settled effect result. The next committed decision
consumes that input exactly once.

An `OutcomeUnknown` operation remains active and blocks later commands. A host
may explicitly call `Kernel::abandon_unknown_effect` with the exact operation
and frozen runtime. The kernel asks the loop to close its own checkpoint and
semantic history, then atomically fails the operation and releases the session.
The effect itself remains `OutcomeUnknown` with no fabricated outcome. The same
request is idempotent after a lost reply. Supplying a confirmed external result
is still outside v0 because no adapter can yet produce that evidence.

## SQLite and ownership

SQLite is the only v0 backend. There is no storage trait. Connections enable
foreign keys, WAL, `synchronous=FULL`, and a bounded busy timeout.

One `Kernel` holds an operating-system lock on a sidecar of the canonical
database path for its lifetime. A second cooperating kernel writer fails to
open the same database. The database directory is a trusted host boundary;
the sidecar cannot fence an actor that deliberately ignores it. Inside the
owning process, a session driver lease prevents two concurrent `drive` calls
for one session. SQLite state, not wakeups or in-memory ownership, remains
authoritative after restart.

The database `user_version` and every operation state version fail closed when
newer than supported. Opening the kernel also checks all foreign-key
relationships. Unknown phases, malformed identities, manifest data, checkpoint
data, or impossible cross-record relationships are corruption; they never
trigger an external effect.

## Transaction and crash matrix

No loop plugin or effect adapter runs inside a SQLite transaction.

| Boundary | Atomic durable write | Reopened meaning | Permitted next action |
| --- | --- | --- | --- |
| Create agent | agent identity | agent exists | exact retry is a no-op |
| Create session | session plus agent binding | empty isolated session | exact retry is a no-op |
| Submit command | command, operation, next command position | queued exact input | return original admission or activate in order |
| Activate | active pointer, frozen manifest, `NeedDecision` | one active ordered operation | call loop with durable input |
| Commit semantic decision | checkpoint, events, cursor, `NeedDecision` | events and next loop position agree | call loop again |
| Commit effect intent | checkpoint, exact effect, `EffectIntent` | adapter definitely not started | mark dispatch started |
| Mark dispatch | effect dispatch count, `EffectDispatched` | adapter may have started | invoke now, or recover by class |
| Settle effect | exact outcome, effect `Settled`, operation `NeedDecision` | result is available exactly once | call loop; never repeat settled effect |
| Mark uncertainty | effect and operation `OutcomeUnknown` | recovery or the live adapter cannot prove the result | block without dispatch |
| Abandon uncertainty | loop checkpoint and events, operation `Failed`, clear active pointer | the operation is closed while the effect remains unknown | return the same outcome on retry or activate queued work |
| Request cancellation | stable cancellation identity and exact active target | cancellation is authoritative even if the signal reply is lost | signal the exact live operation or close it on the next drive |
| Close cancellation | loop checkpoint and events, operation `Cancelled`, clear active pointer | effect facts remain definite, not dispatched, or unknown as recorded | exact request retry is a no-op or activate queued work |
| Terminate | checkpoint, events, outcome, clear active pointer | operation is terminal | activate next queued operation |

Required deterministic injections cover both sides of activation, effect
intent, dispatch, effect completion, settlement, unknown-effect abandonment,
cancellation closure, and terminal event commits. A panic or process loss after
a committed row never requires inference from a missing record.

## Foundation proof

The first complete slice must prove through the public seams:

1. agent and session isolation;
2. stable command admission and changed-content conflict;
3. ordered activation with queued content kept out of semantic history;
4. one lifetime database writer and one active driver per session;
5. intent and dispatch are durable before adapter invocation;
6. exact safe replay after a possibly dispatched crash;
7. unsafe possibly dispatched work becomes explicitly unknown;
8. settlement, next state, and effect result are atomic;
9. a settled effect is never repeated;
10. runtime manifests are frozen and mismatches fail before execution;
11. checkpoint schema mismatches fail before state advances;
12. semantic event replay is gapless and rejects an ahead cursor;
13. newer database or stored-state versions fail closed;
14. a live adapter can report uncertainty without creating a false failure;
15. dropping a drive cancels its adapter but retains session and database
    ownership until cleanup finishes; and
16. explicit unknown-effect abandonment validates the frozen runtime and
    gapless history, never invokes an adapter, is idempotent after a lost reply,
    preserves the effect as unknown, and releases queued work atomically; and
17. cancellation is persisted before signalling, targets one exact active
    operation, prevents later dispatch when it wins, waits for started work to
    stop, preserves the effect's certainty, closes loop-owned history, and
    releases queued work atomically.

The scripted loop and fake effect adapter are test boundaries only. They are
not product policy or privileged kernel implementations.

## First external runtime consumer

`renoa-agent-loop` depends on both the kernel and the provider-neutral
`renoa-agent` vocabulary. It reconstructs model history from versioned semantic
message events, stores only its operational program counter in the opaque
checkpoint, and expresses every model and tool call as a named kernel effect.
`renoa-local` resolves the Pi model, durable context strategy, instructions,
and concrete workspace tools into the first complete local Host runtime. Its
contract is recorded in [`renoa-host-v0.md`](renoa-host-v0.md). This preserves
the intended dependency direction:

```text
renoa-kernel
    ^
renoa-agent-loop
    ^
renoa-local / future hosts and surfaces
```

No provider, tool, prompt, workspace, or agent-loop type was added to the
kernel schema or API for this consumer.

## Future session trees and placement

The following constraints are settled design direction, not claims about a v0
API or schema:

- a session remains one linear ordered execution branch;
- branching creates another Session under the same Agent at one committed
  semantic-event cursor rather than introducing concurrent heads inside a
  session;
- a child receives an immutable semantic prefix, not its parent's active
  operation, pending effects, checkpoint, runtime manifest, workspace,
  connections, or credentials;
- branch identity uses stable Agent, Session, and event identities rather than
  database row numbers, file paths, node addresses, or surface connections;
- surface attachment, execution placement, workspace placement, and session
  identity remain separate; and
- future execution transfer must copy and verify durable state before switching
  authority, then fence the previous executor before activation.

V0 adds no parent-session field, fork command, node identifier, placement
record, transfer package, or merge operation. Those contracts require a real
fork or movement path plus idempotence, isolation, recovery, and fencing tests.

## Locked decisions

1. The kernel is a mandatory privileged durability layer, not an optional
   plugin.
2. Agent variants are runtime compositions; the kernel has no agent-kind
   branch.
3. A session is linear with one active operation and gapless local positions.
4. Admitted data is committed before acknowledgement and stable identities are
   bound to exact content.
5. Runtime compatibility is frozen per operation and checked before loop or
   effect execution.
6. The loop is decision-only and cannot access kernel storage or external
   capabilities through its interface.
7. Every external action has an exact durable intent and an explicit dispatch
   boundary.
8. Unsafe possibly dispatched effects become unknown and never replay
   automatically.
9. Effect settlement and the next durable program-counter state are atomic.
10. Semantic events and operational recovery state are separate journals.
11. SQLite is the only v0 store and one process owns it exclusively.
12. Provider, tool, surface, workspace, and product policy stay outside the
    kernel.
13. Unknown schema and state versions fail closed before any external action.
14. Unknown effects remain blocked until an explicit host action; abandonment
    can close loop-owned history but cannot change the effect into a definite
    success or failure.
15. Cancellation is an idempotent, exact-operation durable command. It is
    committed before signalling, cannot start new effects, and does not erase
    whether existing work settled, never dispatched, or may have run.

## Explicitly open decisions

- authoritative settlement of `OutcomeUnknown` from an adapter receipt,
  callback, or status lookup tied to the stable effect identity;
- steering, approval, and their ordering relative to commands and cancellation;
- RCP task-to-session provisioning and coordinator command positions;
- event retention, deletion, indexing, and snapshot compaction;
- kernel-enforced checkpoint size limits;
- explicit cross-session context sharing, including provenance,
  authorization, captured source position, and reference-versus-materialized
  representation;
- effect-level idempotence and fencing contracts stronger than
  `SafeToReplay`;
- workspace checkpoints, executor quiescence, and host migration;
- durable child-session creation at a committed event cursor, including its
  idempotence and semantic-prefix representation; and
- whether a second real store or harness ever proves a narrower shared storage
  boundary.

An open decision becomes locked only when a runtime consumes it, tests prove
its invariants on the real path, and this document records the reason.

Host-level model-context compaction is implemented and tested as replaceable
loop policy. It remains outside the kernel ownership boundary; only a future
kernel-enforced checkpoint-size limit is still open here.
