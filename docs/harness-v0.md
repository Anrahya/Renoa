# Renoa Harness architecture v0

## Status and authority

This document defines Renoa's durable agent-harness architecture. The
standalone model-and-tool slice exists in `renoa-harness`: it durably admits
and orders operations, persists each model or tool intent before dispatch,
settles complete results atomically, records uncertain effects honestly,
durably cancels active standalone operations, and recovers from process
interruption using SQLite. Runtime profiles can project immutable history into
model context, create durable bounded checkpoints before context overflow, and
inspection exposes known usage without hiding missing or uncertain attempts.

`renoa-local` is the first product-path host. It combines that harness with Pi
AI provider routing and external local read, edit, write, and process tools. It
can complete and durably continue a coding conversation without adding those
implementations or their policy to the harness core.

RCP-bound admission, approvals, steering, and host movement remain later
slices. Sections describing them constrain those slices; they do not claim
that those behaviors exist today.

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
- context projection and durable compaction;
- frozen runtime configuration for active work;
- durable token usage and user-visible output; and
- recovery after process interruption.

A runtime profile may supply the Agent SDK's `ContextProjector`. Before a
checkpoint exists, Renoa passes it a copy of complete durable history before
each new model intent. Once a checkpoint is active, the harness first builds
the checkpointed context view and passes that view to the projector. Projection
does not rewrite history. A projection failure leaves the operation at the
same retryable boundary, and durable cancellation prevents provider dispatch.
The projector is trusted host code and must not perform externally visible
effects; compaction summaries are separate durable harness effects instead.
Changing projector behavior requires a new runtime-profile revision.

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

Each tool binding supplies one conservative recovery class before an intent is
committed. Renoa requires the host to choose it explicitly; there is no hidden
default. A broad tool such as Bash must remain `NeverReplay` unless the entire
binding is safe to repeat. Invocation-specific classification may be added only
when a real tool can prove it from the exact call.

The current standalone slice has one explicit mode: every registered tool is
allowed, and an unregistered tool is unavailable. It has no approval request,
policy callback, or hidden permission abstraction. Approval is added only when
a real node policy and surface consume it.

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

Cancellation is the first durable **control request**, not a queued operation.
It has its own stable identity and target, and the session owner applies it
without placing it behind later user commands. Its durable row proves admission;
the targeted operation's state and output prove terminal application. Future
approval resolution and steering follow the same control category only when
implemented. Operation positions count only user commands. RCP ordering among
different control types remains deferred to their integration slices.

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

The implemented standalone slice uses one SQLite backend with these logical
records:

1. **Sessions** — identity, admission and output cursors, and active operation.
2. **Operations** — immutable admission data plus one versioned, mutable total
   current state; queued work is an operation in `Queued` state.
3. **Conversation entries** — immutable, linearly ordered user and assistant
   content associated with an operation.
4. **Model-attempt records** — attempt status, known token usage, uncertainty,
   and the exact request retained only while that attempt is recoverable.
5. **Tool-call records** — the unresolved source-ordered batch, exact call,
   reserved result identity, recovery class, and current effect identity.
6. **Cancellation requests** — stable request identity and exact active
   operation target.
7. **Context checkpoints and attempts** — immutable prefix summaries plus each
   exact persisted compactor request, settlement token, usage, and uncertainty.
8. **Output records** — append-only user-visible facts carrying their producing
   operation, stable record IDs, and a gapless session-local sequence.

RCP admission mode and task binding plus workspace binding are added only when
their implementation slices consume them. Settled tool results are immutable
conversation entries; transient tool-call records are deleted when their batch
is completely resolved.

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
state carry format versions; an unknown newer version fails closed. One
explicit v1-to-v2 migration preserves model-only databases created before
durable tools; v2-to-v3 adds cancellation requests without rebuilding tool
state; v3-to-v4 adds stable binding identities and fails closed for legacy
in-flight tool work that cannot identify its implementation; v4-to-v5 adds
durable context checkpoints without changing queued command identity. Renoa
has no generic migration framework.

Every external request identity is bound to immutable content. An exact retry
returns the existing admission; reuse with different content is a conflict.

## Operation state machine

The current state contains everything needed to choose the next action without
replaying a mutation log or inferring meaning from a missing row.

The minimal conceptual phases are:

```text
Queued
  -> NeedModel
  -> CompactionPending -> NeedModel
  -> ModelPending
  -> NeedTool -> WaitingForApproval -> ToolPending
  -> NeedModel | Completed

any active phase -> OutcomeUnknown
any active phase -> Failed | Cancelled   (only after safe settlement)
```

`WaitingForApproval` is a future phase reachable only before tool intent. It
will retain the exact tool continuation. Implemented cancellation is a durable
request alongside the current phase, not a replacement phase that forgets a
pending effect.

The implemented state persists the model-attempt count, active tool-batch
identity and cursor, and frozen limits so restart cannot reset a safety bound.
Every model attempt and tool invocation has a harness-generated effect ID and
settlement token. A separate logical step number is not stored before an
observer or policy consumes it.

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
the implemented standalone slice, the supplied profile resolves the model
adapter and tool implementations while the harness durably freezes the system
instructions, stable tool-binding identities, tool specifications, recovery
declarations, and safety limits.
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

A profile without tools advertises none and rejects a provider tool call before
inserting it. A tool-enabled profile validates the complete response before
settlement, commits the assistant response and full tool plan together, and
refuses a batch above its frozen limit.

## Tool execution and transcript validity

Every tool intent stores the exact call and arguments, reserved result
identity, effect ID, settlement token, and recovery class. The operation state
holds the frozen runtime revision, stable host-supplied tool-binding identity,
and exact advertised tool specification. Recovery accepts a tool only when all
three match; a binding implementation change requires a new identity. Legacy
tool work without an identity fails closed after migration.

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
resolution. The implemented tool slice includes one minimal `abandon unknown`
control: it appends an honest error result saying that the pending call's
outcome is unknown and it was not retried, appends skipped-without-execution
results for the remaining calls in that batch, and fails the operation. It
never guesses that the external effect failed. Richer reconciliation remains
future work.

A tool emits zero or more transient progress updates followed by exactly one
final result. Durable harness v0 executes each model-produced batch
sequentially. Unknown tool names and tool failures become model-visible error
results. Calls from a length-stopped response are never executed; the complete
batch receives error results before any continuation. Parallel batches remain
an in-memory Agent feature until crash tests prove independent recovery and
source-ordered settlement.

Before another model request or operation activates, every settled assistant
tool call has exactly one durable result in source order, including denied,
unavailable, cancelled-before-start, and reconciled interrupted calls.

The harness may be temporarily structurally incomplete while paused on an
in-flight call, but it never presents that transcript to a model. A failed or
cancelled operation needs no fabricated conversation message; its existing
entries remain history and its terminal status is exposed through durable
output.

## Approval and cancellation

Approval remains a later feature. Cancellation is implemented for the active
operation of a standalone session.

An approval request is persisted before any surface sees it. It has a stable
identity, and the first authorized resolution wins idempotently across devices.
Approval occurs before tool intent and does not make an unsafe tool replayable.
RCP transports the question and answer; node policy remains authoritative. In
the current all-allowed mode, registering a tool is the host's permission to
run it and no approval state exists.

`request_standalone_cancellation` first commits a caller-stable cancellation ID
bound to the exact active operation, then signals its in-process driver. Exact
retries are idempotent, reuse against another target conflicts, and queued or
already-terminal operations reject a new request. A cancellation committed
before terminal settlement wins that race.

Model cancellation retains no partial assistant message. A complete response
that arrives after the request records known usage but does not enter the
conversation. A cancelled or interrupted model attempt may still have incurred
remote cost, so unknown usage remains unknown.

Tool cancellation waits for the tool future to confirm that all work it owns
has stopped; there is no timeout that starts the next operation early. A
process tool must kill and reap its process group before returning. The current
call retains its known final result—a cancellation result when it was
stopped—and every unstarted call in its batch receives a source-ordered
not-executed result before the operation terminates.
If the harness process disappears while a tool is pending, it cannot prove the
effect stopped: recovery records `OutcomeUnknown` even for a normally
`SafeToReplay` binding and performs no replay.

Each cancellation settlement consumes or invalidates the pending state under
the existing effect ID and settlement token checks. A late completion cannot
settle output. Dropping an execution future is not ordered cancellation.

## Observation and RCP publication

The implemented standalone API exposes a diagnostic `inspect()` snapshot with
the complete conversation, operation statuses, per-operation model usage, and
one terminal durable output per finished operation. Usage reports the sum of
known provider counts together with total attempts, attempts lacking usage,
and attempts whose outcome is unknown; the latter counts may overlap. It does
not turn unknown usage into zero. `inspect()` does not yet expose cursor reads,
a subscription, or transient streaming and is not the future RCP bridge API.

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
derived projection. Without a host projector, the active checkpoint view is
used directly. With one, the host projector runs over that view before a new
model intent. The exact resulting `ModelRequest` is committed before sampling.
Recovery of a dispatched attempt reuses that request without rerunning either
checkpoint construction or host projection.

Compaction is not deletion. It creates an immutable checkpoint that summarizes
one contiguous prefix of the durable transcript and atomically selects it for
future model context. The original entries remain inspectable and are the
authority if a checkpoint is wrong. A checkpoint contains only its identity,
its previous checkpoint, the last transcript sequence it covers, and its
summary. It does not copy the retained tail.

The provider-neutral context order is:

```text
frozen system instructions and tool specifications
checkpoint summary, when one is active
exact active-operation user request, when covered by the checkpoint
complete transcript entries after the checkpoint
host context projection
```

The active user request remains exact even when a checkpoint advances through
part of its operation. An assistant tool-call message and every corresponding
tool result form one indivisible cut group. The harness prefers completed
operation boundaries, then completed tool groups within the active operation.
It never exposes an orphan tool result or a tool call whose results were cut
away.

Mutable workspace facts, project instructions, running processes, plans, and
similar host state are not facts in the summary format. A host that needs them
reinjects a current snapshot through its projector. The generic harness does
not learn coding-specific checkpoint fields.

### Context budget

A compaction-enabled runtime profile freezes an explicit model context window,
reserved response space, post-compaction target, maximum checkpoint size, and
bounded per-checkpoint retry count. It also supplies one deterministic,
effect-free request sizer whose estimate cannot decrease when content is only
added. This monotonic contract lets the harness find safe cut points without
constructing every possible growing prefix. The harness has no provider or
model table and no silent default context size.

Before every model dispatch, including a continuation after tool results, the
harness constructs and sizes the exact candidate request. It may dispatch only
when:

```text
estimated input <= context window - reserved response space - safety margin
```

The target after compaction is materially lower than that dispatch boundary so
one checkpoint creates useful headroom instead of causing another checkpoint
on the next step. Concrete model values are host configuration and are tuned by
evaluation rather than hard-coded into the kernel.

If the sizer determines that the frozen instructions, tool specifications,
host projection, and exact active user request cannot fit without historical
entries, compaction cannot solve the request. The operation fails with an
explicit context-capacity outcome before provider dispatch; Renoa does not
silently truncate user intent. If an approximate sizer misses this case, a
typed pre-inference provider rejection triggers the same check and the rejected
request is never dispatched again unchanged.

Token sizing is conservative but cannot be assumed exact for every provider.
A model adapter may classify a provider rejection as definite context overflow
only when it knows inference did not begin. That rejection is recorded as a
completed attempt without fabricated usage, forces one durable compaction, and
is never replayed unchanged. If output already started, the shared sampler
downgrades the error to outcome-unknown. Unknown transport or stream failures
remain outcome-unknown and never trigger a supposedly free retry from
error-string guessing in the harness. A provider rejection of the compactor
request itself fails once instead of replaying a known-oversized request.

`renoa-local` resolves context and provider output limits from Pi's selected
model at startup. It requests at most 32,768 output tokens, reserves that cap
plus the larger of 8,192 tokens or two percent of the context window, targets
60 percent of the remaining dispatch budget after compaction, allows a
checkpoint up to the smaller of 16,384 tokens or one quarter of that target,
and permits two summary attempts per checkpoint. Its deterministic estimator
counts text and JSON at three UTF-8 bytes per token, adds message/tool framing,
and assigns a bounded 4,096-token image estimate. These are host defaults, not
kernel constants; explicit provider overflow remains the correctness fallback
because no local heuristic is an exact provider tokenizer.

Pi's own context guard normally reuses usage attached to historical assistant
messages. A Renoa checkpoint can replace that original prefix, and Renoa's
provider-neutral messages do not carry ordering timestamps that make the old
usage safe to reuse. The Pi adapter therefore passes historical usage as zero
to Pi while preserving the real usage in Renoa's durable telemetry; both Pi
and the harness estimate the request that is actually sent.

### Checkpoint construction

The harness chooses a safe transcript prefix that leaves the configured recent
tail and fits the compactor request. If all eligible history cannot fit into
one compactor request, it advances through bounded prefix chunks and rebuilds
the candidate request after each committed checkpoint. It never drops oldest
input merely to make its own summary request fit.

One unusually large tool result is represented to the compactor by separately
labelled bounded head and tail text plus its omitted size and digest; the full
durable result is unchanged. Images are represented by stable metadata rather
than copied base64.
If one indivisible exact user input cannot fit, the capacity rule above wins.

The same frozen model initially performs compaction with no tools. Its system
instruction treats the embedded transcript as untrusted data and requires one
concise checkpoint with these sections:

- goal and user intent;
- hard constraints and preferences;
- completed work;
- current state and blockers;
- decisions and their rationale;
- exact working facts such as paths, symbols, commands, and errors; and
- next action and unresolved questions.

The summary omits full source files, superseded facts, hidden reasoning, and
temporary environment state. A completed response is accepted only when it
stops normally, contains no tool calls, has non-empty required sections, and
fits the checkpoint budget. A length stop or malformed summary is a completed
but rejected compaction attempt whose known usage remains observable.

Provider-native compaction may later become an adapter capability after a
second implementation proves the shared boundary. The durable transcript and
portable checkpoint remain authoritative; opaque provider state does not enter
the v0 harness schema.

### Durable compaction effect

Compaction uses the same persistence rule as every other external effect:

1. construct and size the candidate context;
2. choose one safe covered-through sequence;
3. commit the exact summary request, prior checkpoint identity, reserved new
   checkpoint identity, effect identity, settlement token, and frozen retry
   state;
4. invoke the model without tools;
5. validate the complete response; and
6. in one transaction, record usage, insert the immutable checkpoint, compare
   and replace the expected active checkpoint, and return the operation to
   `NeedModel`.

The next loop rebuilds and resizes context. It may create another bounded
checkpoint before committing the normal model intent.

A crash after summary dispatch but before settlement has unknown billing and
completion. Recovery records that uncertainty and may repeat the exact saved
request only within its frozen budget. A stale effect or settlement token
cannot activate a checkpoint. Cancellation records any known completed usage
but does not activate a checkpoint for cancelled work. A rejected, failed, or
cancelled compaction leaves the previous active checkpoint and full transcript
unchanged.

Compaction usage contributes to the operation's existing model-usage totals;
the harness does not report internal model work as free. Summary quality is
tested through continuation tasks, not by asserting that generated prose looks
plausible.

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
18. Compaction creates immutable, replaceable projections and never rewrites
    the authoritative transcript.
19. A checkpoint preserves an exact active user request and structurally
    complete recent model/tool groups outside its summary.
20. Context is sized before every provider dispatch against frozen host-supplied
    limits; locally detected irreducible requests fail before dispatch, and a
    typed pre-inference overflow is never dispatched again unchanged.
21. Checkpoint creation is a persisted model effect with honest usage,
    uncertainty, cancellation, and stale-result handling.
22. Product and workspace state are reinjected by the host rather than encoded
    in the general checkpoint format.

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
- restart preserves attempt counts, tool-batch cursors, and safety limits;
- an unexpected tool call fails closed without entering tool state; and
- required SQLite durability settings are verified on the actual connection.

### Tool slice

- every crash position around a tool intent and result is injected;
- safe calls replay with exact saved input;
- recovery skips settled calls and resumes the remaining batch in source order;
- unsafe pending calls pause without starting later work;
- abandoning an unknown unsafe call writes honest, source-ordered results and
  unblocks the session without replaying the effect;
- changed specifications, recovery declarations, and unavailable tool
  revisions fail closed;
- invalid persisted batch cursors fail before dispatch;
- every settled call receives exactly one source-ordered result; and
- failed or explicitly abandoned operations restore a provider-valid
  transcript before later activation.

### Cancellation slice

- the request commits before the live driver is signalled;
- exact request retries are safe and one identity cannot target two operations;
- queued and terminal operations reject new cancellation requests;
- cancellation wins every model and tool settlement race when its transaction
  commits first;
- cancelling a model writes no partial assistant message and restart does not
  redispatch it;
- cancelling a tool waits for confirmed shutdown, never starts remaining batch
  calls, and restores source-ordered transcript validity;
- process loss with a pending tool remains `OutcomeUnknown` and never becomes
  a fabricated cancellation; and
- retrying an old cancellation cannot signal a newer active operation.

### Compaction slice

- the first request below the frozen boundary dispatches without compaction;
- the first request above it commits a checkpoint before the normal model
  intent and retains the exact active user request;
- cut selection never separates assistant tool calls from their results;
- one large active turn can checkpoint completed early tool groups while
  retaining its recent suffix;
- a compactor input too large for one request advances through bounded chunks
  without deleting transcript entries;
- a giant tool result is bounded only in the compactor view while inspection
  returns its complete durable content;
- crashes before intent, after intent, after dispatch, before checkpoint
  settlement, and after settlement reopen to one explicit state;
- a stale summary cannot replace a newer active checkpoint;
- cancelled compaction records known usage but cannot activate its checkpoint;
- malformed, tool-shaped, truncated, failed, and outcome-unknown summary
  attempts consume the frozen retry budget honestly;
- checkpoint plus recent-tail context remains provider-valid after restart;
- repeated checkpoints carry the prior summary into the next update request;
  contradiction and supersession quality remains part of the real-provider
  continuation evaluation.

A real long-running provider-backed coding continuation across at least three
checkpoints remains the final quality evaluation; deterministic crash and
continuation proofs do not substitute for it.

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

Approval and executor migration gain their own race tests when each feature is
implemented; they are not retroactive prerequisites for the implemented
slices.

## Implementation order

1. **Implemented:** the standalone model-only SQLite foundation shares the
   one-attempt `sample_model` primitive with `renoa-agent`; a pre-cancelled
   attempt is rejected before the adapter is invoked.
2. **Implemented:** durable sequential tools share one invocation primitive,
   freeze exact bindings, and recover through `SafeToReplay`, `NeverReplay`, or
   explicit abandonment.
3. **Implemented:** durable standalone cancellation uses a stable targeted
   request, ordered model/tool shutdown, and terminal transcript repair.
4. Add approval only with a real node policy and surface; the current registered
   tool set is intentionally all-allowed.
5. **Implemented:** `renoa-local` proves the product boundary with Pi AI model
   routing plus external read, edit, write, and bash tools; it is an
   all-allowed personal host, not a hostile sandbox.
6. **Implemented:** optional host context projection preserves full history,
   persists the exact projected request before dispatch, and keeps usage for
   failed and uncertain work observable without inventing zeroes.
7. **Implemented:** durable context checkpoints use bounded incremental input,
   exact effect persistence, cancellation, provider-overflow fallback, honest
   usage, and deterministic repeated-continuation tests. The exact requested
   real-provider evaluation remains pending model availability. Add steering
   only when a live consumer defines its ordering behavior.
8. Add cursor-based observation and the thin RCP/ACP adapters only after the
   local harness can complete real work safely.
9. Design workspace checkpoints and host movement only after local continuity
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
- Durable approval and steering operation shapes, plus the mapping from an RCP
  cancellation command to the implemented standalone cancellation identity.
- Product defaults beyond the implemented Pi-backed local host, and the
  evidence threshold for enabling a future provider-native compaction adapter.
- Workspace identity, snapshots, executor quiescence, and tool fencing.
- Retention and deletion policy.

An open choice becomes locked only when code consumes it, deterministic tests
prove the invariant, and this document records the reason.

## Reference evidence

No upstream source is incorporated. The design was informed by:

- Pi `origin/main` at `581d75a89cea21e50d6a26df840352f94427f633`
  (MIT): safe compaction cut points, exact retained tails, split-turn handling,
  and structured summary updates. Its durable harness compaction remains
  unfinished, so Renoa does not treat it as recovery evidence.
- Pi AI `0.84.1` at `53fa77ccd8a279eb87e92294ef3687b03ff80112`
  (MIT): the adapter consumes its model catalog and exported explicit-overflow
  classifier. Renoa's durable state machine and request estimator are its own;
  no Pi source is copied into the harness.
- OpenAI Codex CLI `origin/main` at
  `357696c5e7127525a9259d3dcfa0574516b1fe84` (Apache-2.0): concise handoff
  checkpoints, retained user anchors, mid-turn triggering, and a separate
  provider-native compaction path. Renoa does not depend on that provider path.
- Grok Build `origin/main` at
  `e5fd4816d43260c15ba785f103990c1ed6cea230` (Apache-2.0): deterministic
  reinjection of instructions, the latest request, recent history, and live
  coding state plus validation of full-replacement summaries. Renoa keeps the
  reinjection principle but not its coding-specific full-replacement engine.
- Cursor's published cloud-agent, Remote Control, and My Machines design:
  separated conversation, agent execution, and workspace state. Renoa does not
  adopt its proprietary services, Temporal, or cloud-VM infrastructure.
