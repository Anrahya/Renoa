# RCP JSON/WebSocket binding v0

## Status

This document maps the [RCP operation contract](rcp-operations-v0.md) onto the
first implemented transport binding. The current binding version is `7`.

The binding is a candidate contract, not a stable public release. Version `7`
is implemented by `renoa-control`, `renoa-node`, a headless TypeScript surface,
and a TypeScript Pi node. Cross-language tests cover both authenticated roles,
discovery and authorization, replay, live reattachment, offline-node rejection,
lost acknowledgements, `replay_required`, JavaScript's exact integer limit, and
a real Pi OpenAI-compatible turn.

## Connection

- Endpoint: `/connect`
- Transport: WebSocket
- Application frames: one UTF-8 JSON object per text frame
- Top-level discriminator: `type`
- Variant and top-level field names: `snake_case`
- UUIDs: lowercase hyphenated strings
- Optional top-level values are encoded as `null`, not omitted

The first application frame must be `enroll` or `authenticate`. Enrollment
returns credentials and ends that connection. Authentication returns
`authenticated` and keeps the connection open for operations.

The current server accepts plaintext `ws://` only on a loopback listener. Any
public deployment must terminate TLS and use `wss://` before credentials are
sent.

## Binding version

The client sends `version` only while enrolling or authenticating. The server
rejects any value other than `7` with `version_mismatch` and ends the session.
Once authenticated, later operation frames do not repeat the version.

The binding version covers framing, JSON shape, and error vocabulary. A change
to operation semantics, field meaning, or serialized shape requires a new
binding version unless it is explicitly defined as compatible.

Version `7` supersedes version `6` by removing harness configuration from RCP
commands and execution delivery. Version `6` is rejected on new sessions;
stored commands and task events are migrated without changing their durable
identities. The shared execution-event profile introduced in version `6`
remains unchanged.

## Session establishment

Enrollment request:

```json
{
  "type": "enroll",
  "version": 7,
  "token": "<single-use enrollment secret>"
}
```

Enrollment response:

```json
{
  "type": "enrolled",
  "version": 7,
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
  "version": 7,
  "credentials": {
    "deviceId": "00000000-0000-0000-0000-000000000001",
    "credential": "<device secret>"
  }
}
```

Successful authentication:

```json
{
  "type": "authenticated",
  "version": 7
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
      "target": "workspace:renoa"
    }
  ]
}
```

An authorized principal with no tasks receives an empty `tasks` array. Version
`7` defines no pagination or live directory update frame.

### Attach

```json
{
  "type": "attach",
  "request_id": 41,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "after_sequence": 12
}
```

Use `null` for `after_sequence` to request the full journal.

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

`request_id` correlates one connection attempt. `command_id` is the durable
idempotency identity and must be reused with identical content after an
uncertain response.

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

The other current record kind is `execution_event`. Its nested value uses the
same `ExecutionEvent` shape shown in the node publication below. It is shared by
the Rust and Pi nodes and contains complete durable activity, not token deltas.

## Node frames

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

`request_id`, task sequences, execution sequences, and millisecond timestamps are JSON
integers. Cross-language clients must keep unsigned values between `0` and
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
- A node reconnect receives every still-pending `execute` delivery.
- A surface reconnects with its last durably applied task sequence.
- A slow attached surface receives `replay_required` and must reattach.
- Clients must not infer task completion from a closed socket.

## Binding exclusions

Version `7` defines no task-list pagination, live directory updates, heartbeat,
cancellation, steering, approval, artifact, binary-frame, compression,
HTTP/SSE, webhook, or public TLS deployment contract. Adding any of those
requires an operation contract first, then a binding and tests.
