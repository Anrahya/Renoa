# RCP JSON/WebSocket binding v0

## Status

This document maps the [RCP operation contract](rcp-operations-v0.md) onto the
first implemented transport binding. The current binding version is `14`.

The binding is a candidate contract, not a stable public release. The current
version is implemented by `renoa-control`, `renoa-node`, the TypeScript headless and
browser surfaces, and a TypeScript Pi node. Cross-language tests cover both authenticated roles,
discovery and authorization, replay, live reattachment, offline-node rejection,
lost acknowledgements, `replay_required`, JavaScript's exact integer limit, and
a real Pi OpenAI-compatible turn.

## Connection

- Endpoint: `/connect`
- Transport: WebSocket
- Application frames: one UTF-8 JSON object per text frame
- Maximum reassembled application message: 1 MiB
- Top-level discriminator: `type`
- Variant and top-level field names: `snake_case`
- UUIDs: lowercase hyphenated strings
- Optional top-level values are encoded as `null`, not omitted

The first application frame must be `enroll`, `authenticate`, or
`authenticate_ticket`. Enrollment returns credentials and ends that connection.
Either authentication frame returns `authenticated` and keeps the connection
open for operations.

The client must send that first application frame within 10 seconds after the
WebSocket upgrade. The coordinator closes a connection that misses the
deadline or exceeds the message limit. Large artifacts belong outside this
control binding; the limit prevents task frames from becoming an unbounded
bulk-transfer path.

The reference coordinator admits at most 128 simultaneous upgraded
connections. An attempt beyond that process-wide budget receives HTTP `503`
before the WebSocket upgrade. This is an implementation resource bound, not a
task or device limit.

The coordinator sends an empty WebSocket `ping` control frame every 30 seconds.
Peers answer with the standard `pong`; browsers do this automatically. Control
frames are ignored by the JSON operation decoder. They carry no RCP identity,
ordering, or durable state and exist only to keep idle proxy paths alive.

The current server accepts plaintext `ws://` only on a loopback listener. Any
public deployment must terminate TLS and use `wss://` before credentials are
sent. The first deployment does this at `wss://renoa.live/connect` through an
outbound tunnel; the tunnel is deployment infrastructure, not part of the
binding.

## Binding version

The client sends `version` only while enrolling or authenticating. The server
rejects any value other than `14` with `version_mismatch` and ends the session.
Once authenticated, later operation frames do not repeat the version.

The binding version covers framing, JSON shape, and error vocabulary. A change
to operation semantics, field meaning, or serialized shape requires a new
binding version unless it is explicitly defined as compatible.

Version `14` supersedes version `13` by adding the chat essentials. A task
summary carries a `status`. `attach` carries a required `live` flag. A surface
can `stop_command`, answered by `stop_accepted`; the coordinator delivers `stop`
to the node, which answers `acknowledge_stop`. A node can
`publish_live_activity`, which the coordinator relays to interested surfaces as
`live_activity`. A version `13` peer is rejected at authentication.

Version `13` superseded version `12` by adding an optional `author` to text
command input, which marks text written by a guest rather than the
authenticated principal. A version `12` peer is rejected at authentication.

Version `12` superseded version `11` by adding an optional `context` to text
command input: the surface's own description of where the text was written.
A version `11` peer is rejected at authentication.

Version `11` superseded version `10` by adding task deletion by the task's
owner (`delete_task`). A version `10` peer is rejected at authentication.

Version `10` superseded version `9` by adding node target advertisement
(`advertise_targets`) and runtime task opening by the node's owner
(`list_targets`, `open_task`). A version `9` peer is rejected at authentication.

Version `9` superseded version `8` by adding one-use browser connection-ticket
authentication. The ticket contains no identity claim: its principal and
surface were bound by the coordinator when the ticket was issued.

Version `8` superseded version `7` by adding stable `commandId` causation to
durable `execution_event` task records. Existing execution records are migrated
by joining their execution identity to the coordinator's durable command
binding without changing task-event identities or sequences.

Version `7` superseded version `6` by removing harness configuration from RCP
commands and execution delivery. Versions `6` and `7` are rejected on new sessions;
stored commands and task events are migrated without changing their durable
identities. The shared execution-event profile introduced in version `6`
remains unchanged.

## Session establishment

Enrollment request:

```json
{
  "type": "enroll",
  "version": 14,
  "token": "<single-use enrollment secret>"
}
```

Enrollment response:

```json
{
  "type": "enrolled",
  "version": 14,
  "credentials": {
    "deviceId": "00000000-0000-0000-0000-000000000001",
    "credential": "<device secret>"
  }
}
```

Authentication request:

```json
{
  "type": "authenticate",
  "version": 14,
  "credentials": {
    "deviceId": "00000000-0000-0000-0000-000000000001",
    "credential": "<device secret>"
  }
}
```

Browser ticket authentication request:

```json
{
  "type": "authenticate_ticket",
  "version": 14,
  "ticket": "<60-second single-use connection secret>"
}
```

The ticket is sent only in this first frame, never in a URL. The coordinator
deletes it atomically before successful authentication. A consumed or expired
ticket fails with `authentication_failed`; reconnecting requires a new ticket.
The passkey HTTP ceremony that issues it is defined in
[identity-v0.md](identity-v0.md).

Successful authentication:

```json
{
  "type": "authenticated",
  "version": 14
}
```

The client never sends a principal, surface role, or node role during
authentication. Those values come from the durable device enrollment.

## Surface frames

### List tasks

```json
{
  "type": "list_tasks",
  "request_id": 40
}
```

The response contains only tasks owned by the authenticated principal, ordered
by `taskId`:

```json
{
  "type": "task_list",
  "request_id": 40,
  "tasks": [
    {
      "taskId": "00000000-0000-0000-0000-000000000010",
      "target": "workspace:renoa",
      "status": "working"
    }
  ]
}
```

`status` is `working`, `idle`, or `failed`, derived from the task's latest
command as the operation contract defines. An authorized principal with no
tasks receives an empty `tasks` array. The binding defines no pagination or
live directory update frame.

### List targets

```json
{
  "type": "list_targets",
  "request_id": 41
}
```

The response contains the targets currently advertised by online nodes that the
authenticated principal owns, ordered by `nodeId` then `target`:

```json
{
  "type": "target_list",
  "request_id": 41,
  "targets": [
    {
      "nodeId": "00000000-0000-0000-0000-000000000030",
      "target": "agent:arcee"
    }
  ]
}
```

### Open a task

```json
{
  "type": "open_task",
  "request_id": 42,
  "task_id": "00000000-0000-0000-0000-000000000011",
  "node_id": "00000000-0000-0000-0000-000000000030",
  "target": "agent:arcee"
}
```

The surface generates `task_id` before the first send and reuses it on retry.
Success, including an exact retry, returns:

```json
{
  "type": "task_opened",
  "request_id": 42,
  "task_id": "00000000-0000-0000-0000-000000000011"
}
```

A node the principal does not own and a target the node does not advertise
return `not_found`; an offline node returns `node_offline`; a task identity
already bound differently returns `conflict`.

### Delete a task

```json
{
  "type": "delete_task",
  "request_id": 43,
  "task_id": "00000000-0000-0000-0000-000000000011"
}
```

Success, including a retry for a task already deleted, returns:

```json
{
  "type": "task_deleted",
  "request_id": 43,
  "task_id": "00000000-0000-0000-0000-000000000011"
}
```

A task with an execution in flight returns `conflict`; another principal's task
returns `not_found`.

### Attach

```json
{
  "type": "attach",
  "request_id": 41,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "after_sequence": 12,
  "live": true
}
```

Use `null` for `after_sequence` to request the full journal. `live` is
required: `true` asks for `live_activity` frames while the task's turns run,
and `false` receives only the journal. A frame without `live` is not a version
`14` attach.

The first successful response identifies the replay high-water mark:

```json
{
  "type": "attached",
  "request_id": 41,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "through_sequence": 18
}
```

`through_sequence` is `null` for an empty task. Zero or more `task_event`
frames follow for the durable suffix, then the same frame type carries live
records.

### Live activity

An attachment that sent `"live": true` receives frames like this one while the
task's node runs a turn:

```json
{
  "type": "live_activity",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "activity": { "type": "text_delta", "sequence": 0, "text": "Hel" }
}
```

`sequence` counts from zero for each message of the turn, and `text` is the
visible assistant text since the previous frame. This frame abandons what the
surface showed for the message in progress:

```json
{
  "type": "live_activity",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "activity": { "type": "text_reset" }
}
```

A live frame has no `request_id`, no task sequence, and no durable effect. The
`assistant_message` record that completes the text supersedes the live text. A
surface that is too slow to take a frame loses it and is not told to replay
because of it. Text that arrives within 50 milliseconds of the previous frame is
sent with the message's next fragment of any kind once that time has passed, or
not at all when the durable record comes first.

A frame the node published before a record is delivered before that record, on
the same connection and in the node's order, unless it was dropped; a dropped
frame never arrives later, so a client never waits for a missing one. A frame
published just after a record may be delivered before it too.

### Stop a command

```json
{
  "type": "stop_command",
  "request_id": 44,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

The coordinator answers after the stop is durable:

```json
{
  "type": "stop_accepted",
  "request_id": 44,
  "command_id": "00000000-0000-0000-0000-000000000020",
  "outcome": "requested"
}
```

`outcome` is `requested` or `already_ended`. A command already ended with no
earlier stop records nothing and answers `already_ended`; a retry of a recorded
stop answers `requested` again. A task the principal does not own and a
command the task never admitted return `not_found`. The command's terminal
`execution_event` carries the result, `cancelled` when the stop won.

### Submit

```json
{
  "type": "submit",
  "request_id": 42,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "input": {
    "type": "text",
    "text": "continue the implementation"
  }
}
```

Successful durable admission:

```json
{
  "type": "command_accepted",
  "request_id": 42,
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

Text input may carry an optional `context` string, written by the surface to
say where the text was written, for example
`"context": "Discord server 1\nchannel #general (2)"`. It is omitted when
absent. RCP stores and delivers it with the command and gives it no other
meaning; the executor decides whether to show it to the agent.

Text input may also carry `"author": "guest"` when the surface received the
text from someone other than its principal, such as another member of a shared
Discord channel. It is omitted for the principal's own text; any other value,
`"principal"` included, is rejected. The coordinator authenticates only the surface's principal
and carries the author as the surface asserts it; the executor decides what a
guest's command may do. The Renoa Host runs it with no `USER.md`, no plugin
context, and every tool call refused; the Pi node fails it without running.

RCP reserves no words in text. The Renoa node reads the principal's text that is
exactly `/compact` (surrounding whitespace ignored) as its compaction control
and ends the command with one fixed `assistant_message` and `completed`;
`/compact` followed by more words ends `failed`, and a guest's is an ordinary
message (see `rcp-v0.md`). The Pi node treats all of it as ordinary text.

`request_id` correlates one connection attempt. `command_id` is the durable
idempotency identity and must be reused with identical content, `context`
and `author` included, after an uncertain response.

### Task event

```json
{
  "type": "task_event",
  "event": {
    "eventId": "00000000-0000-0000-0000-000000000030",
    "taskId": "00000000-0000-0000-0000-000000000010",
    "sequence": 19,
    "kind": {
      "type": "command_submitted",
      "command": {
        "commandId": "00000000-0000-0000-0000-000000000020",
        "principalId": "00000000-0000-0000-0000-000000000050",
        "surface": "mac",
        "target": "workspace:renoa",
        "input": {
          "type": "text",
          "text": "continue the implementation"
        }
      }
    }
  }
}
```

The other current record kind is `execution_event`:

```json
{
  "type": "task_event",
  "event": {
    "eventId": "00000000-0000-0000-0000-000000000031",
    "taskId": "00000000-0000-0000-0000-000000000010",
    "sequence": 20,
    "kind": {
      "type": "execution_event",
      "commandId": "00000000-0000-0000-0000-000000000020",
      "event": {
        "eventId": "00000000-0000-0000-0000-000000000060",
        "executionId": "00000000-0000-0000-0000-000000000070",
        "sequence": 0,
        "recordedAtMs": 1786137600000,
        "kind": { "type": "execution_started" }
      }
    }
  }
}
```

`commandId` is the stable causation link to the admitted command. The nested
value uses the same `ExecutionEvent` shape shown in the node publication below.
It is shared by the Rust and Pi nodes and contains complete durable activity,
not token deltas.

## Node frames

After authentication, a node advertises the targets it can execute for newly
opened tasks. Each advertisement replaces the connection's previous targets and
has no acknowledgement:

```json
{
  "type": "advertise_targets",
  "targets": ["agent:alpha", "agent:arcee"]
}
```

An invalid advertisement returns `invalid_message` with a `null` `request_id`.

The coordinator delivers an admitted command:

```json
{
  "type": "execute",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command": {
    "commandId": "00000000-0000-0000-0000-000000000020",
    "principalId": "00000000-0000-0000-0000-000000000050",
    "surface": "mac",
    "target": "workspace:renoa",
    "input": {
      "type": "text",
      "text": "continue the implementation"
    }
  }
}
```

The delivery deliberately contains no harness configuration. The node adapter
uses `task_id` and the opaque command target to select its local harness. Model,
instructions, tools, permissions, and provider configuration do not appear on
the wire.

After local durable admission, the node sends:

```json
{
  "type": "acknowledge_execution",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

The committed response is:

```json
{
  "type": "execution_acknowledged",
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

The node publishes one contiguous activity batch:

```json
{
  "type": "publish_execution_events",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "events": [
    {
      "eventId": "00000000-0000-0000-0000-000000000060",
      "executionId": "00000000-0000-0000-0000-000000000070",
      "sequence": 0,
      "recordedAtMs": 1786137600000,
      "kind": {
        "type": "execution_started"
      }
    }
  ]
}
```

The execution is already bound to the admitted command by the operation's
`command_id`; the start event does not duplicate that command. A committed
response reports the source cursor:

```json
{
  "type": "execution_events_accepted",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "through_execution_sequence": 0
}
```

The coordinator delivers a recorded stop, after the pending `execute` frames on
a node's activation and whenever a stop is recorded for an online node:

```json
{
  "type": "stop",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

After the node durably records the stop and cancels the command, it sends:

```json
{
  "type": "acknowledge_stop",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020"
}
```

There is no success response. A node that does not own the task receives
`invalid_message`. Until the acknowledgement commits, the coordinator repeats
the `stop`.

While a turn runs, the node may publish live activity. The coordinator relays
it as `live_activity` to the attached surfaces that asked, only when the node
owns the task, and sends no response:

```json
{
  "type": "publish_live_activity",
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000020",
  "activity": { "type": "text_delta", "sequence": 0, "text": "Hel" }
}
```

Node frames use `command_id` for correlation because it is already the durable
operation identity. They do not carry `request_id`.

## Errors

```json
{
  "type": "error",
  "request_id": 42,
  "code": "conflict",
  "message": "command id was already used with different content"
}
```

`request_id` is `null` when no surface request can be correlated. Valid codes
are:

- `authentication_failed`
- `invalid_message`
- `invalid_role`
- `node_offline`
- `not_found`
- `conflict`
- `internal`
- `replay_required`
- `version_mismatch`

An `internal` response uses the generic message `internal coordinator error`.
Storage and serialization details are not exposed on the wire.

## Numbers

`request_id`, task sequences, execution sequences, live text sequences, and
millisecond timestamps are JSON integers. Cross-language clients must keep unsigned values between `0` and
`9,007,199,254,740,991`, and signed timestamps between
`-9,007,199,254,740,991` and `9,007,199,254,740,991`. The coordinator rejects an
incoming frame outside this range as `invalid_message`. It replaces an
unrepresentable outbound frame with a generic `internal` error and closes that
connection rather than sending a number another implementation may silently
round.

## Ordering and recovery

- WebSocket preserves frame order on one connection; it does not make frames
  durable.
- `command_accepted`, `execution_acknowledged`, and `execution_events_accepted` are
  sent only after their documented commits.
- A node reconnect receives every still-pending `execute` delivery, then every
  stop it has not acknowledged.
- `stop_accepted` is sent only after the stop is committed. `live_activity` is
  sent without any commit and may be lost. A node sends the `publish_live_activity`
  frames it has queued before the `publish_execution_events` it has read, and the
  coordinator sends the `live_activity` frames waiting for an attachment before
  the `task_event` it forwards.
- A surface reconnects with its last durably applied task sequence.
- A slow attached surface receives `replay_required` and must reattach.
- Clients must not infer task completion from a closed socket.

## Binding exclusions

The binding defines no task-list pagination, live directory or target updates,
application heartbeat or presence, steering, approval, artifact, binary-frame,
compression, HTTP/SSE, webhook, or public TLS deployment contract, and no live
activity beyond visible text. Adding any of those requires an operation contract first, then a
binding and tests. The transport `ping` above is not an application heartbeat.
