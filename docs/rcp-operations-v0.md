# RCP operation contract v0

## Status

This document defines the transport-independent behavior implemented by the
current Renoa Continuity Protocol proof. It is subordinate to
[rcp-v0.md](rcp-v0.md), which remains the canonical architecture contract.

This is not yet a stable public standard. The behavior below is intentional and
tested. The command and baseline activity payloads are consumed by both Renoa's
Rust executor and a Pi SDK node, but remain provisional until RCP is publicly
deployed.

## Boundary

RCP operations preserve task continuity. They do not define an agent loop,
model API, system prompt, tool or permission system, context policy, user
interface, or network transport.

Enrollment and authentication establish a trusted session. They are not task
operations. Creating tasks, issuing enrollment tokens, and revoking devices are
trusted control-plane actions and are not remotely callable RCP operations in
the current proof.

After authentication, the session identity fixes the peer's role:

- a surface may `ListTasks`, `Attach`, and `Submit`;
- a node may `AcknowledgeExecution` and `PublishExecutionEvents`;
- the coordinator may deliver `Execute` to a node and `TaskEvent` records to an
  attached surface.

The transport may add correlation data around an operation. Correlation data is
not durable identity and must not affect admission or deduplication.

## Shared rules

Every implementation must preserve these rules:

1. The authenticated session supplies principal, surface, or node identity.
   An operation cannot override that identity.
2. Task authorization is checked on every operation, not only when the session
   is opened.
3. A durable acknowledgement is sent only after the acknowledged mutation is
   committed.
4. Retries reuse stable command or event identities. A transport request ID may
   change on every attempt.
5. The coordinator assigns one gapless sequence to committed records within a
   task. Source event order and task journal order remain separate.
6. Delivery is at least once. Exact retries are harmless; reuse of an identity
   with different content is a conflict.
7. A connection ending never terminates a task or proves an execution outcome.

## Surface operations

### `ListTasks`

`ListTasks` has no semantic input. The coordinator derives the principal from
the authenticated surface session and returns `TaskList` containing zero or
more summaries authorized for that principal.

Each current summary contains only:

- `task_id`: the durable identity needed by later operations;
- `target`: the existing target reference used to distinguish tasks.

Results are ordered by task identity so independent clients receive a
deterministic snapshot. Node identity, harness configuration, presence, cursor
state, and execution status are not exposed.

The operation is read-only and not paginated. A retry returns the current
authorized snapshot, which may include tasks created since the earlier request.
It is not a live directory subscription.

### `Attach`

Inputs:

- `task_id`: the durable task to observe;
- `after_sequence`: the last contiguous task sequence already applied by the
  surface, or no cursor for a full replay.

Preconditions:

- the peer is a surface;
- the task belongs to the surface's authenticated principal;
- a supplied cursor is not ahead of the task's durable high-water mark.

Behavior:

1. The coordinator subscribes to live records before loading the durable
   suffix.
2. It captures the task high-water mark and returns `Attached` with that value.
3. It returns committed `TaskEvent` records after the supplied cursor through
   the captured high-water mark.
4. It then emits live records above that mark.

This ordering closes the replay-to-live race. An exact retry is read-only. A
surface that falls behind the live buffer receives `ReplayRequired`, reconnects,
and attaches from its last durably applied sequence.

### `Submit`

Inputs:

- `task_id`: the target task;
- `command_id`: a stable identity generated before the first send;
- `input`: the command payload. The current profile supports text input only.

The coordinator derives principal and surface identity from the authenticated
session and target and execution node from durable task state. A surface cannot
provide or replace those values.

For a new command, the coordinator atomically commits:

- the normalized command;
- its `CommandSubmitted` task record;
- its pending execution delivery.

Only then does it return `CommandAccepted`. The current product policy rejects a
new command with `NodeOffline` when its bound node is unavailable.

An exact retry of an already admitted command returns `CommandAccepted` even if
the node is now offline. Current node availability cannot rewrite the result of
past durable admission. Reusing the command identity with different content,
task, surface identity, or target returns `Conflict`.

## Executor delivery and node operations

### `Execute`

`Execute` is a coordinator-to-node delivery containing only the task identity
and normalized command. It may be delivered more than once until the node
acknowledges durable local admission.

The node must deduplicate the command in its harness ledger before model calls
or side effects. Receiving `Execute` is authority to attempt the command; it is
not evidence that the command started or completed.

The receiving adapter resolves the task's harness configuration locally. RCP
does not carry or interpret an agent identifier, instructions, model selection,
tools, or capability policy. The Rust and Pi adapters consume this same
delivery despite using different local agent configuration and execution code.

### `AcknowledgeExecution`

Inputs:

- `task_id`;
- `command_id`.

The node sends this only after its local harness has durably admitted the
command. The coordinator verifies that the authenticated node owns the task,
deletes the pending delivery, commits that deletion, and returns
`ExecutionAcknowledged`.

The operation is idempotent. If its response is lost, the node repeats the same
task and command identities.

### `PublishExecutionEvents`

Inputs:

- `task_id`;
- `command_id`;
- one non-empty, contiguous batch of harness-neutral `ExecutionEvent` values.

The baseline activity profile requires:

- sequence zero to be `ExecutionStarted`;
- every batch to contain one execution identity and contiguous source sequences;
- a terminal event, when present, to be last;
- no new event after the execution is terminal.

The coordinator accepts exact overlap with already committed events. Previously
accepted sequence positions must keep the same event identity and content. A
gap, mutation, changed execution identity, invalid start, or post-terminal event is
rejected.

New events become `TaskEvent` records with coordinator-assigned task sequences
in the same transaction that advances the accepted source cursor. After commit,
the coordinator returns `ExecutionEventsAccepted` with the highest durable
execution sequence. A lost response is recovered by resending an overlapping
batch.

The current durable event kinds are `ExecutionStarted`, `TurnStarted`,
`AssistantMessage`, `ToolStarted`, `ToolFinished`, and `ExecutionTerminated`.
They are the intersection proven by Renoa's Rust engine and Pi. Token deltas,
provider metadata, usage, reasoning blocks, and native harness messages are not
durable RCP records.

## Durable task records

The current task journal contains two record kinds:

- `CommandSubmitted`, containing the normalized command;
- `ExecutionEvent`, containing one event from the baseline activity profile.

Every record has a stable event identity, task identity, coordinator-assigned
task sequence, and payload. Surfaces deduplicate by stable identity and apply by
task sequence. A timestamp never determines order.

## Acknowledgement meanings

| Outcome | What is durable before it is sent |
| --- | --- |
| `TaskList` | No mutation; the returned tasks were selected by authenticated principal |
| `Attached` | No mutation; authorization and a replay high-water mark were resolved |
| `CommandAccepted` | Command, command record, and pending execution delivery |
| `ExecutionAcknowledged` | Removal of the pending coordinator delivery |
| `ExecutionEventsAccepted` | New task records and the node's accepted source cursor |

None of these outcomes means that an external side effect happened exactly
once.

## Errors

| Code | Meaning |
| --- | --- |
| `authentication_failed` | Session identity could not be established |
| `invalid_message` | The operation or its ordering data violates the contract |
| `invalid_role` | The authenticated role cannot perform the operation, or a node session has been replaced |
| `node_offline` | Current policy refused a new command because its bound node was unavailable |
| `not_found` | The authorized task or command does not exist; task ownership failures use the same result |
| `conflict` | A stable identity was already bound to different content or another execution |
| `internal` | The coordinator could not complete its own work; transport responses must not expose storage details |
| `replay_required` | Live delivery was lost; attach again from the last durable cursor |
| `version_mismatch` | The selected transport binding version is unsupported |

Authentication and version failures belong to session establishment. The other
codes describe operation outcomes. Whether an error carries a request
correlation value is a binding concern.

## Retry rules

| Interaction | Safe retry identity | Result |
| --- | --- | --- |
| `ListTasks` | No durable identity; request correlation is transport-local | Returns the current authorized snapshot |
| `Attach` | task ID plus last applied sequence | Reconstructs the same suffix, then resumes live delivery |
| `Submit` | command ID plus identical content | Returns the existing admission without another task record |
| `Execute` delivery | command ID | Node harness admits one local execution identity |
| `AcknowledgeExecution` | task ID plus command ID | Leaves no pending delivery and confirms again |
| `PublishExecutionEvents` | execution ID, event IDs, and source sequences | Accepts exact overlap and appends only the new contiguous suffix |

## Deliberately undefined

Task-list pagination, live directory updates, user-facing task metadata,
cancellation, steering, approvals, task rebinding, executor generations,
snapshots, artifact transfer, and transient token streaming are not operations
yet. Harness provisioning and permission policy stay outside RCP. A generic
durable interaction flow for approval or follow-up responses remains undefined
until a harness and surface consume it.
