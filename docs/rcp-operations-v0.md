# RCP operation contract v0

## Status

This document defines the transport-independent behavior implemented by the
current Renoa Continuity Protocol proof. It is subordinate to
[rcp-v0.md](rcp-v0.md), which remains the canonical architecture contract.

This is not yet a stable public standard. The behavior below is intentional and
tested. The command and baseline activity payloads are consumed by both Renoa's
Rust Host and a Pi SDK node. A real Alpha tool turn and two-surface continuation
have crossed the public TLS origin, but the contract remains provisional while
identity administration and production node operation are unfinished.

## Boundary

RCP operations preserve task continuity. They do not define an agent loop,
model API, system prompt, tool or permission system, context policy, user
interface, or network transport.

Enrollment and authentication establish a trusted session. They are not task
operations. Issuing enrollment tokens and recording a node's owning principal
are trusted control-plane actions and are not remotely callable RCP operations.
The operator may revoke any device. An owner may also list and revoke their own
signed-in apps with `ListDevices` and `RevokeDevice`, from a browser or a device
that proved its key; devices are not tasks, and these operations grant no task
authority. From a device that proved its key an owner may call their Host with
`CallHost`, which the coordinator relays to the node that serves it; a Host
call is not a task and enters no journal. An operator may also create a task directly. The node's owner can
open a task remotely with `OpenTask`, but only against a target its online node
currently advertises, and delete one of its tasks with `DeleteTask`.

After authentication, the session identity fixes the peer's role:

- a surface may `ListTasks`, `ListTargets`, `OpenTask`, `Attach`, `Submit`,
  `StopCommand`, and `DeleteTask`; a surface connected by a browser ticket or a
  proven device key may also `ListDevices` and `RevokeDevice`, and one connected
  by a proven device key may `CallHost`;
- a node may `AdvertiseTargets`, `ListTargets`, `SendAgentMessage`,
  `AcknowledgeExecution`, `PublishExecutionEvents`, `AcknowledgeStop`,
  `PublishLiveActivity`, `ServeHostControl`, and `CompleteHostCall`;
- the coordinator may deliver `Execute`, `Stop`, and `HostCall` to a node, and
  `TaskEvent` records and `LiveActivity` frames to an attached surface.

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
8. Live activity is advisory. It is not acknowledged, retried, stored, or
   replayed, and nothing a surface needs after reconnection depends on it.

## Surface operations

### `ListTasks`

`ListTasks` has no semantic input. The coordinator derives the principal from
the authenticated surface session and returns `TaskList` containing zero or
more summaries authorized for that principal.

Each current summary contains only:

- `task_id`: the durable identity needed by later operations;
- `target`: the existing target reference used to distinguish tasks;
- `status`: what the task is doing, derived from its latest command.

The status is `working` while the latest command's execution has not
terminated, `idle` once it completed or was cancelled, and `failed` once it
failed. A task with no command is `idle`. "Latest" is the order the
coordinator admitted the commands in, so an older command whose execution never
terminated does not keep a task `working` after a later command has run. The
status is a function of the task journal: a surface holding the journal can
compute the same value, and the directory is how a surface that holds none of it
learns the value. A surface may show finer states of its own, such as a command
that is admitted but not started; those are display states, not statuses. It
does not say whether the node is online, whether the owner
is being waited on, or when the task next wakes; nothing in RCP produces those
facts yet.

Results are ordered by task identity so independent clients receive a
deterministic snapshot. Node identity, harness configuration, presence, and
cursor state are not exposed.

The operation is read-only and not paginated. A retry returns the current
authorized snapshot, which may include tasks created since the earlier request.
It is not a live directory subscription.

### `ListTargets`

`ListTargets` has no semantic input. The coordinator returns `TargetList`: every
target currently advertised by an online node that the caller's principal owns.
A surface's principal is its own; a node's is its owner, so an agent learns
which other agents it may message. A node without an owner is refused with
`InvalidRole`. Each summary contains the node identity, the opaque target
reference, and the target's display name, ordered by node and target.

The result reflects live connections. A node's targets disappear when its
connection ends and return when it reconnects and advertises again. The
operation is read-only; a retry returns the current snapshot.

### `OpenTask`

Inputs:

- `task_id`: a stable identity generated by the surface before the first send;
- `node_id`: the node that will execute the task;
- `target`: one target that node advertised.

The coordinator derives the principal from the authenticated session. In one
transaction it checks, in order:

1. If the task identity already exists with the same principal, node, and
   target, the opening is an exact retry and returns `TaskOpened`, even if the
   node is now offline. Any other existing binding returns `Conflict`.
2. The node must belong to the principal; otherwise `NotFound`.
3. The node must be online (`NodeOffline`) and advertising the target
   (`NotFound`).

Only then does it commit the task and return `TaskOpened`. A rejected opening
leaves no task. The node creates the task's harness session when it executes the
task's first command.

### `Attach`

Inputs:

- `task_id`: the durable task to observe;
- `after_sequence`: the last contiguous task sequence already applied by the
  surface, or no cursor for a full replay;
- `live`: whether the surface wants live activity (see `LiveActivity`) while
  the task's turns run. A required flag, not a default: a surface that renders
  only the journal sends false.

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

An attachment that asked for live activity also receives `LiveActivity` frames
from the task's node. Frames are delivered on a buffer separate from the
journal's. A surface too slow to take a frame loses that frame; it is never
told to replay because the frame buffer overran. A frame never waits for room:
frames and journal records share the connection's outbound queue, and a frame
that finds it full is dropped, so a surface that stops reading holds at most one
queue of frames and no record waits behind more of them. A surface that falls
behind the journal's own buffer is still told to replay.

Before the coordinator forwards a journal record to an attachment it forwards
the frames already waiting for it, and a node sends the frames it has queued
before the execution records it has read. A frame the node published before a
record therefore reaches the surface before that record unless it was dropped.
A frame published just after a record may also reach the surface before it.

### `LiveActivity`

`LiveActivity` is a coordinator-to-surface delivery containing the task
identity, the causing `command_id`, and one activity:

- `text_delta`: a `sequence` and the visible assistant `text` produced since
  the previous frame. Sequences count from zero for each message of the turn,
  so a surface can tell that it missed a frame;
- `text_reset`: a message the surface was shown part of was abandoned, so the
  surface discards the text it showed for it.

A frame has no task sequence and no acknowledgement, and a surface that
reconnects is never sent the frames it missed. The durable `AssistantMessage`
of the same text supersedes the live text: a surface replaces what it showed
with the record. Reasoning and tool-call arguments are never live activity.

### `Submit`

Inputs:

- `task_id`: the target task;
- `command_id`: a stable identity generated before the first send;
- `input`: the command payload. The current profile supports text input only,
  with an optional `context`: the surface's own description of where the text
  was written, and an optional guest `author` for text the surface received
  from someone other than its principal. RCP carries both with the command and
  gives them no other meaning. A surface cannot supply an `origin`; only the
  coordinator sets one, on an agent message.

The coordinator derives principal and surface identity from the authenticated
session and target and execution node from durable task state. A surface cannot
provide or replace those values.

For a new command, the coordinator atomically commits:

- the normalized command;
- its `CommandSubmitted` task record;
- its pending execution delivery.

Only then does it return `CommandAccepted`. A new command whose bound node is
unavailable is rejected with `NodeOffline` before admission and leaves no
record; the sender resends it once the node returns.

An exact retry of an already admitted command returns `CommandAccepted` even if
the node is now offline. Current node availability cannot rewrite the result of
past durable admission. Reusing the command identity with different content,
task, surface identity, or target returns `Conflict`.

### `StopCommand`

Inputs:

- `task_id`: a task the authenticated principal owns;
- `command_id`: the command to stop. The command is the stable identity of the
  stop.

Preconditions:

- the peer is a surface;
- the task belongs to the surface's authenticated principal;
- the command was admitted for that task. Otherwise the result is `NotFound`,
  the same as an unknown or foreign task.

Behavior:

1. If a stop was already recorded for the command, the coordinator returns its
   original answer, `requested`, even when the command has ended since.
2. Otherwise, if the command's execution has terminated, it returns
   `already_ended` and records nothing.
3. Otherwise it commits a stop record, returns `StopAccepted` with `requested`,
   and delivers `Stop` to the task's node if the node is online.

A stop is accepted while the node is offline. `Submit` and `OpenTask` refuse
an absent node because they add work for it; a stop adds none, and a Host turn
can outlive its node's link to the coordinator. The stop waits for the node
and follows its pending executions on reconnect.

The stop has no outcome record of its own. The result is the command's terminal
record in the journal: `cancelled` when the stop reached the execution first,
otherwise whatever terminal the execution had already reached. A stop never
reaches another command of the task.

### `DeleteTask`

Inputs:

- `task_id`: a task the authenticated principal owns.

The coordinator deletes the task, its journal, its commands and their execution
state in one transaction, then returns `TaskDeleted`. Nothing is archived. A task with a command whose
execution has not terminated, including one pending delivery or acknowledged
without events yet, is refused with `Conflict` and left intact. Another
principal's task returns `NotFound`. Deleting a task that no longer exists
returns `TaskDeleted`, so a retry after a lost reply converges; its identity may
then be opened again. The node that executed the task deletes its own copy of
the task separately.

### `ListDevices`

`ListDevices` has no semantic input. The coordinator derives the principal from
the authenticated session and returns `DeviceList`: the principal's active
signed-in apps, each with its identity, surface, label, creation time and
whether it is the device that asked, oldest first. Only devices that registered a
key are listed. The operation is read-only. A surface that authenticated with a
bearer credential alone, such as a chat surface's service link, is refused with
`InvalidRole`.

### `RevokeDevice`

Inputs:

- `device_id`: a signed-in app of the authenticated principal.

In one transaction the coordinator marks the device revoked. Only then does it
return `DeviceRevoked` and end the device's open connections; a later
authentication of its credential fails. Revoking a device the principal already
revoked returns `DeviceRevoked` again, so a retry converges. Another
principal's device, an unknown device and a device without a registered key
return `NotFound`. A device may revoke itself; its acknowledgement is sent
before its connection closes. The key of a revoked device cannot sign in again.

### `CallHost`

Inputs:

- `method`: `GET`, `POST` or `PUT`;
- `path`: `/v1/host` or a path under it, with no query;
- `body`: the request's JSON text, or none for a `GET`.

The coordinator sends the call, with the authenticated principal, to the one
connected node of that principal that serves Host control, and returns the
node's answer as `HostAnswer`: an HTTP status and JSON text, the Host's result or
its own refusal. It checks only the path's form, the body's size and that a
`GET` has none; it knows no Host operation and stores neither the call nor the
answer. A browser ticket connection, a surface that authenticated with a bearer
credential alone, and a principal that owns no node are refused with
`InvalidRole`. When no node or two nodes serve the Host, or the node does not
answer, the call returns `NodeOffline` and its outcome is unknown.

## Executor delivery and node operations

### `Execute`

`Execute` is a coordinator-to-node delivery containing only the task identity
and normalized command. It may be delivered more than once until the node
acknowledges durable local admission.

The node must deduplicate the command in its harness ledger before model calls
or side effects. Receiving `Execute` is authority to attempt the command; it is
not evidence that the command started or completed.

A command that another agent caused carries an `origin` (see
`SendAgentMessage`). The adapter decides how its agent is told who the message
is from; RCP gives the origin no other meaning.

The receiving adapter resolves the task's harness configuration locally. RCP
does not carry or interpret an agent identifier, instructions, model selection,
tools, or capability policy. The Rust and Pi adapters consume this same
delivery despite using different local agent configuration and execution code.

### `AdvertiseTargets`

Inputs:

- `targets`: the cards the node can execute for newly opened tasks. A card is
  an opaque `target` and the `name` the owner and other agents call it.

The coordinator replaces the targets of the node's current connection. A node
that never advertises offers no targets, but still executes the tasks already
bound to it. Each target and each name must be 1 to 256 bytes without control
characters, targets must be unique, and a node may advertise at most 256. A
node shortens and cleans a longer or unclean name before advertising it, so its
own agent is never left out. An invalid advertisement is rejected with
`InvalidMessage` and changes nothing.

There is no acknowledgement. Advertisements are presence, not durable state:
the coordinator keeps them only for the life of the connection.

### `SendAgentMessage`

Inputs:

- `from_command`: the command the sending agent is executing;
- `message_id`: a stable identity generated before the first send;
- `node_id` and `target`: the receiving agent, as `ListTargets` shows it;
- `text`: what to tell it, 1 byte to 64 KiB and not blank.

The coordinator derives the sender: `from_command` must belong to a task bound
to the authenticated node, whose owner is the principal. In one transaction it
checks, in order:

1. The node has an owner (`InvalidRole`).
2. `from_command` is one this node is executing (`NotFound`).
3. The text is not blank and fits the limit (`InvalidMessage`).
4. The message would not be deeper than 100 in its chain (`InvalidMessage`).
   A message sent while executing a command that has an origin is one deeper
   than that origin; any other is depth 1. The refusal does not state the limit.
5. The receiving node belongs to the same principal (`NotFound`) and the
   receiving agent is not the sender (`InvalidMessage`).
6. If `message_id` already exists with identical content, the admission is an
   exact retry and returns `CommandAccepted`, even if the receiving node is now
   offline or the sender's name has changed. Any other existing content returns
   `Conflict`.
7. The sending agent is advertised by its node, which supplies its name
   (`InvalidMessage`).
8. The receiving node is online (`NodeOffline`) and advertises the target
   (`NotFound`).

Only then does it commit the thread task if it does not yet exist, the command
with its `CommandSubmitted` record, and its pending execution delivery, and
return `CommandAccepted`. The thread's identity derives from the principal and
both agents, so every message from one agent to another lands in one task. A
rejected message leaves no task, command, or record. The command's surface is
`agent` and its `origin` carries `from_command`, the sender's target and name,
the depth, and the kind `request`.

The reason of each refusal above, and of a reused message identity, is written
for the agent that will read it: whole sentences that say what was wrong and
what to do next, with no identity or internal term of the coordinator. A
`SendAgentMessage` or a `ListTargets` on a connection that a newer connection
of the same node has replaced is refused with `InvalidRole` and carries its own
`request_id`, so the node's wait ends with the reason.

When the execution of a request reaches a terminal event, the same transaction
that records the event admits one answer command in the task that holds
`from_command`. Its identity derives from the request, so a retried publication
admits no second answer. The answer's kind is `reply` for a completed execution,
carrying its last assistant message, and `failure` for a failed or cancelled
one, carrying the reason. An answer's text is cut to the 64 KiB a request may
carry. Its origin names the answering agent and is one
deeper than the request. It is delivered like any command, and waits as a
pending delivery while the asker's node is offline. An answer is not itself
answered and is not refused for depth. If the asking command no longer exists,
no answer is admitted.

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

### `Stop`

`Stop` is a coordinator-to-node delivery containing a task identity and a
command identity. The coordinator sends it when a stop is recorded and the node
is online, and again on every node activation until the node acknowledges it,
after the pending `Execute` deliveries of the same activation.

The node records the stop in its own ledger first. It then cancels the command
through its harness: a running turn is cancelled, and a command it has not
started begins cancelled. A stop that arrives before its `Execute` is still
honored when the command arrives. A stop for a command that already
terminated is acknowledged and changes nothing. The node deduplicates stops by
command identity.

### `AcknowledgeStop`

Inputs:

- `task_id`;
- `command_id`.

The node sends this only after it durably recorded the stop. The coordinator
verifies that the authenticated node owns the task, marks the stop delivered,
and stops redelivering it. A node that does not own the task receives
`InvalidMessage` and changes nothing. An acknowledgement for a task the
coordinator no longer holds has nothing left to confirm and is accepted. There
is no success response; the operation is idempotent, and a node whose
acknowledgement was lost is delivered the stop again and repeats it.

### `PublishLiveActivity`

Inputs:

- `task_id`;
- `command_id`;
- one `text_delta` or `text_reset`.

The coordinator relays the frame as `LiveActivity` to the surfaces attached to
the task that asked for live activity, and only when the authenticated node
owns the task. It stores nothing and sends no response. A frame with no
interested surface, or from a node that does not own the task, is dropped.
The node sends frames only while a turn runs, in visible text order, and never
sends reasoning.

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
in the same transaction that advances the accepted source cursor. Each emitted
record carries the stable `command_id` already bound to the execution so a
surface can project turns safely after replay. After commit, the coordinator
returns `ExecutionEventsAccepted` with the highest durable execution sequence.
A lost response is recovered by resending an overlapping batch.

The current durable event kinds are `ExecutionStarted`, `TurnStarted`,
`AssistantMessage`, `ToolStarted`, `ToolFinished`, and `ExecutionTerminated`.
They are the intersection proven by Renoa's Rust engine and Pi. Token deltas,
provider metadata, usage, reasoning blocks, and native harness messages are not
durable RCP records. Visible text deltas are relayed only as ephemeral
`LiveActivity`.

## Durable task records

The current task journal contains two record kinds:

- `CommandSubmitted`, containing the normalized command;
- `ExecutionEvent`, containing the causing command identity and one event from
  the baseline activity profile.

Every record has a stable event identity, task identity, coordinator-assigned
task sequence, and payload. Surfaces deduplicate by stable identity and apply by
task sequence. A timestamp never determines order.

## Acknowledgement meanings

| Outcome | What is durable before it is sent |
| --- | --- |
| `TaskList` | No mutation; the returned tasks were selected by authenticated principal |
| `TargetList` | No mutation; the returned targets belong to online nodes the principal owns |
| `TaskOpened` | The task and its principal, node, and target binding |
| `Attached` | No mutation; authorization and a replay high-water mark were resolved |
| `CommandAccepted` | Command, command record, and pending execution delivery; for `SendAgentMessage` also the thread task |
| `StopAccepted` (`requested`) | The command's stop record and its pending delivery to the node |
| `StopAccepted` (`already_ended`) | No mutation; the command's execution had already terminated |
| `ExecutionAcknowledged` | Removal of the pending coordinator delivery |
| `ExecutionEventsAccepted` | New task records and the node's accepted source cursor |
| `TaskDeleted` | Removal of the task and everything the coordinator held for it |
| `DeviceList` | No mutation; the returned devices were selected by authenticated principal |
| `DeviceRevoked` | The device's revocation |

None of these outcomes means that an external side effect happened exactly
once.

## Errors

| Code | Meaning |
| --- | --- |
| `authentication_failed` | Session identity could not be established |
| `invalid_message` | The operation or its ordering data violates the contract |
| `invalid_role` | The authenticated role cannot perform the operation, or a node session has been replaced |
| `node_offline` | Policy refused a new command or task opening because the node was unavailable |
| `not_found` | The authorized task, command, node, or advertised target does not exist; ownership failures use the same result |
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
| `ListTargets` | No durable identity; request correlation is transport-local | Returns the current advertised snapshot |
| `OpenTask` | task ID plus identical node and target | Returns the existing opening without another task |
| `AdvertiseTargets` | None; each advertisement replaces the connection's targets | The latest advertisement wins |
| `SendAgentMessage` | message ID plus identical content | Returns the existing admission without another task or record |
| `Attach` | task ID plus last applied sequence | Reconstructs the same suffix, then resumes live delivery |
| `Submit` | command ID plus identical content | Returns the existing admission without another task record |
| `StopCommand` | task ID plus command ID | Returns the original answer without another stop record |
| `DeleteTask` | task ID | Returns `TaskDeleted` whether or not the task still exists |
| `ListDevices` | No durable identity; request correlation is transport-local | Returns the current signed-in apps |
| `RevokeDevice` | device ID | Returns `DeviceRevoked` whether or not this call revoked it |
| `CallHost` | The Host operation's own `operation_id`, for a change | The Host answers a repeated change from its receipt; the coordinator never retries |
| `Execute` delivery | command ID | Node harness admits one local execution identity |
| `AcknowledgeExecution` | task ID plus command ID | Leaves no pending delivery and confirms again |
| `Stop` delivery | command ID | Node records one stop and cancels the command once |
| `AcknowledgeStop` | task ID plus command ID | Leaves the stop delivered |
| `PublishExecutionEvents` | execution ID, event IDs, and source sequences | Accepts exact overlap and appends only the new contiguous suffix |

## Deliberately undefined

Task-list pagination, live directory and target updates, user-facing task
metadata, a waiting-on-owner task status, a next-wake time, steering,
approvals, task rebinding, executor generations, snapshots, artifact transfer,
and live activity beyond visible text are not operations yet. Harness provisioning and permission policy stay outside RCP. A generic
durable interaction flow for approval or follow-up responses remains undefined
until a harness and surface consume it.
