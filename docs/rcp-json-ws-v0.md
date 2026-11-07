# RCP JSON/WebSocket binding v0

## Status

This document maps the [RCP operation contract](rcp-operations-v0.md) onto the
first implemented transport binding. The current binding version is `18`.

The binding is a candidate contract, not a stable public release. The current
version is implemented by `renoa-control`, `renoa-node`, the TypeScript headless and
browser surfaces, and a TypeScript Pi node. The Rust frames and the records they carry are
defined once, in `renoa-rcp-wire`, which the coordinator and every Rust client
share. Cross-language tests cover both authenticated roles,
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
open for operations, except that `authenticate` for a device that registered a
key is answered with `device_challenge` and returns `authenticated` only after a
valid `device_proof` (see Session establishment).

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
rejects any value other than `18` with `version_mismatch` and ends the session.
Once authenticated, later operation frames do not repeat the version.

The binding version covers framing, JSON shape, and error vocabulary. A change
to operation semantics, field meaning, or serialized shape requires a new
binding version unless it is explicitly defined as compatible.

Version `18` supersedes version `17` by adding text input's `attachments`. A
version `17` peer is rejected at authentication. A signed-in app's
[`share_link`](#open-a-shared-file), answered by `share_link_issued`, is a
compatible addition to version `18`: a peer that never sends it sees no
change, and a coordinator that predates it answers it with `invalid_message`
on an authenticated connection rather than refusing the app at sign-in.

Version `17` superseded version `16` by adding sealed setup notices: a signed-in
app's `set_notice_key`, `list_notices` and `acknowledge_notice` (answered by
`notice_key_set`, `notice_list` and `notice_acknowledged`), and a node's
`list_notice_keys`, `put_notice` and `withdraw_notices` (answered by
`notice_keys`, `notice_put` and `notices_withdrawn`). A version `16` peer is
rejected at authentication.

Version `16` superseded version `15` by adding a node's wake request
(`request_wake` answered by `wake_requested`) and text input's `quiet`. A version `15` peer is rejected
at authentication.

Version `15` superseded version `14` by adding push-token registration
(`set_push_token` answered by `push_token_set`, `clear_push_token` answered by
`push_token_cleared`) and a `throughSequence` in every `task_list` summary. A
version `14` peer is rejected at authentication.

Version `14` superseded version `13` by adding four things. The device-key
handshake (`device_challenge`, `device_proof`) and the owner's device operations
(`list_devices`, `revoke_device`, with `device_list` and `device_revoked`). The
chat essentials: a task summary carries a `status`, `attach` carries a required
`live` flag, a surface can `stop_command`, answered by `stop_accepted`; the
coordinator delivers `stop` to the node, which answers `acknowledge_stop`; and a
node can `publish_live_activity`, which the coordinator relays to interested
surfaces as `live_activity`. And Host control relaying (`serve_host_control`,
`host_call`, `complete_host_call`, and for a signed-in app `call_host` answered
by `host_answer`). And agent messages: a node advertises named
targets (`advertise_targets` carries `{target, name}` cards instead of bare
strings), `target_list` summaries gain `name`, `list_targets` is open to nodes
as well as surfaces, a node sends `send_agent_message`, and a command may carry
an `origin`. A version `13` peer is rejected at authentication.

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
  "version": 18,
  "token": "<single-use enrollment secret>"
}
```

Enrollment response:

```json
{
  "type": "enrolled",
  "version": 18,
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
  "version": 18,
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
  "version": 18,
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
  "version": 18
}
```

The client never sends a principal, surface role, or node role during
authentication. Those values come from the durable device enrollment.

### Key-bound devices

A device that signed in with a device key (see
[identity-v0.md](identity-v0.md), "App sign-in and device keys") is answered
with a challenge instead of `authenticated`:

```json
{
  "type": "device_challenge",
  "nonce": "<64 lowercase hex characters, fresh for this connection>"
}
```

The client answers with its signature over
`renoa device proof v1\n<deviceId>\n<nonce>`, as ASN.1 DER base64url without
padding:

```json
{
  "type": "device_proof",
  "signature": "<signature>"
}
```

A valid proof returns `authenticated`, unless the device was revoked, or signed
in again so that the credential in `authenticate` was replaced, while the
connection was proving its key; that session ends without `authenticated`. Any
other next frame, and any proof that
is malformed or does not verify against the registered key, returns
`authentication_failed` and ends the session. `device_proof` is not an
operation: before a challenge it is refused as a first frame
(`invalid_message`), and after authentication it is `invalid_message`. The
challenge and the 10-second authentication deadline cover the whole exchange. A
device without a registered key never sees a challenge.

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
      "status": "working",
      "throughSequence": 18
    }
  ]
}
```

`status` is `working`, `idle`, or `failed`, derived from the task's latest
command as the operation contract defines. `throughSequence` is the task's
last committed sequence, the value `attached` reports as `through_sequence`,
and `null` for an empty task; an app woken by a push attaches only to the tasks
whose `throughSequence` passed its cursor. An authorized principal with no
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
authenticated principal owns, ordered by `nodeId` then `target`. A node may send
the same frame; its principal is its owner, and a node without an owner receives
`invalid_role` with its `request_id`:

```json
{
  "type": "target_list",
  "request_id": 41,
  "targets": [
    {
      "nodeId": "00000000-0000-0000-0000-000000000030",
      "target": "agent:arcee",
      "name": "Arcee"
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

### List devices

```json
{
  "type": "list_devices",
  "request_id": 44
}
```

The response lists the principal's active signed-in apps, oldest first:

```json
{
  "type": "device_list",
  "request_id": 44,
  "devices": [
    {
      "deviceId": "00000000-0000-0000-0000-000000000040",
      "surface": "android",
      "label": "Pixel 9",
      "createdAtMs": 1790000000000,
      "current": true
    }
  ]
}
```

`current` is true for the device that sent the request and false for every
device when a browser asks. Only devices that registered a key are listed; a
node or a service link such as Discord is not. A connection that is neither a
browser ticket nor a device that proved its key receives `invalid_role`.

### Revoke a device

```json
{
  "type": "revoke_device",
  "request_id": 45,
  "device_id": "00000000-0000-0000-0000-000000000040"
}
```

Success, including a retry for a device the principal already revoked, returns:

```json
{
  "type": "device_revoked",
  "request_id": 45,
  "device_id": "00000000-0000-0000-0000-000000000040"
}
```

The acknowledgement is sent after the revocation is committed. The device's open
connections then end, and its next `authenticate` returns
`authentication_failed`. A device may revoke itself; its acknowledgement is
sent before its connection closes. Another principal's device, an unknown
device, and a device without a key return `not_found`; a connection that may not
administer devices receives `invalid_role`.

### Call the Host

A device that proved its key may call the owner's Host, as a browser does under
`/v1/host` ([Host control](#host-control)):

```json
{
  "type": "call_host",
  "request_id": 46,
  "method": "POST",
  "path": "/v1/host/agents",
  "body": "{\"operation_id\":\"00000000-0000-0000-0000-000000000091\",\"preset_id\":\"renoa.app.v5\",\"name\":\"Scout\",\"purpose\":\"Find what is new in my field each week.\"}"
}
```

`method` is `GET`, `POST` or `PUT`. `path` is `/v1/host` or a path under it,
at most 1,024 bytes of visible ASCII, with no query or fragment and no empty,
`.` or `..` segment. `body` is the request's JSON text, or `null`; a `GET`
carries `null`. The coordinator relays a call only when the `host_call` frame
that carries it to the node fits one 1 MiB message, and otherwise refuses it
with `invalid_message`; each Host operation applies its own, smaller limit. The Host's answer,
a result or its own refusal, returns as its HTTP status and JSON text:

```json
{
  "type": "host_answer",
  "request_id": 46,
  "status": 200,
  "body": "{\"operation_id\":\"00000000-0000-0000-0000-000000000091\",\"record\":{}}"
}
```

The call waits on the Host without holding up the connection's other requests,
and ends with the connection: a device revoked meanwhile hears no answer. A
`request_id` must not be reused while its call waits. One connection has at
most eight calls waiting; another is refused with `conflict`. A browser ticket
connection, a service link such as Discord, and a principal that owns no node
receive `invalid_role`. A ticket connection outlives the browser session that
issued it, so a browser calls `/v1/host`, which checks its session each time. A malformed
call receives `invalid_message` and never reaches the Host. When no node of the
owner serves the Host, when two do, or when the call's node disconnects or does
not answer within sixty seconds, the call receives `node_offline`: whether a
change took effect is then unknown, so the caller repeats the same request under
the same `operation_id`. An answer the coordinator cannot relay receives
`internal`; that outcome is unknown too.

### Open a shared file

A device that proved its key asks for a one-time link to a file an agent shared
with its principal (the `/s/{share}` link in the agent's reply):

```json
{
  "type": "share_link",
  "request_id": 47,
  "share_id": "00000000-0000-0000-0000-000000000092"
}
```

```json
{
  "type": "share_link_issued",
  "request_id": 47,
  "path": "/s/00000000-0000-0000-0000-000000000092?ticket=4f0c…"
}
```

`path` is on the coordinator's own origin, and `ticket` is 64 lowercase hex
digits. The coordinator stores only the ticket's digest, with the principal and
the share, for two minutes. Within that time, the first `GET` of the link that
reads the file's first part uses the ticket up and serves the file as a browser
signed in to the Host is served `/s/{share}` ([Shared files](#shared-files));
any later one, one after it expires, and one for another share are refused with
`401`. Asking does not
reach the Host, so a share that does not exist, or is not the principal's, is
refused when the link is opened. Like [`call_host`](#call-the-host), a browser
ticket connection, a service link such as Discord, and a node receive
`invalid_role`.

### Register a push token

A connection authenticated with a device credential registers the device's
push token, so the coordinator can wake the app when it holds no socket:

```json
{
  "type": "set_push_token",
  "request_id": 50,
  "provider": "fcm",
  "token": "fcm-registration-token"
}
```

```json
{
  "type": "push_token_set",
  "request_id": 50
}
```

`provider` is `fcm`; any other value receives `invalid_message` naming it, such
as `provider "gcm" is not supported; expected "fcm"`. `token` is opaque: 1 to
4,096 bytes of printable ASCII without spaces, or `invalid_message` stating the
length received. No error repeats the token. A device holds one token, so a new
one replaces the old, and an exact retry returns `push_token_set` again. A token
another device of the same principal held moves to this device; one another
principal's device holds receives `conflict`
(`token is registered to another principal; sign that device out first`). A
browser ticket connection receives `invalid_role`
(`push tokens belong to an enrolled device; this connection was opened with a
browser ticket`), and so does a node.

```json
{
  "type": "clear_push_token",
  "request_id": 51
}
```

```json
{
  "type": "push_token_cleared",
  "request_id": 51
}
```

Clearing succeeds whether or not the device held a token. Revoking the device
deletes its token too. The wake itself is not an RCP frame: the coordinator
sends the push service one of two constant messages, `changed` or `ring`, with
no task, principal, agent or text in it, and the app learns what changed with
`list_tasks`, `attach` and the Host's feed. A `ring` comes from a node's
[`request_wake`](#wake-the-owner) with urgency `now`.

### Setup notices

A plugin setup link for the owner's app travels as a notice sealed to one
device; the coordinator stores the ciphertext only and cannot read the link. A
connection whose device proved its key registers the key its notices are sealed
to: an uncompressed SEC1 P-256 public key (65 bytes starting `0x04`),
base64url without padding.

```json
{
  "type": "set_notice_key",
  "request_id": 52,
  "public_key": "BHq…"
}
```

```json
{
  "type": "notice_key_set",
  "request_id": 52
}
```

A key that is not such a point receives `invalid_message`. A device holds one
key, so a new one replaces the old. The app lists its device's unexpired
notices, soonest to expire first and at most 23 of them, so the list fits one
1 MiB message. It acknowledges a notice once the owner opened its link or put
it away, and one that does not open on the device at once; acknowledging deletes
it and lets the next list carry the rest:

```json
{
  "type": "list_notices",
  "request_id": 53
}
```

```json
{
  "type": "notice_list",
  "request_id": 53,
  "notices": [
    {
      "notice_id": "00000000-0000-0000-0000-000000000070",
      "sealed": "AQS…",
      "expires_at_ms": 1893456000000
    }
  ]
}
```

```json
{
  "type": "acknowledge_notice",
  "request_id": 54,
  "notice_id": "00000000-0000-0000-0000-000000000070"
}
```

```json
{
  "type": "notice_acknowledged",
  "request_id": 54
}
```

Acknowledging a notice the device does not hold succeeds and changes nothing.
A browser ticket connection, or a device that presented only its bearer
credential, receives `invalid_role` (`setup notices belong to an app that proved
its device key; this connection did not`): whoever holds a notice key receives
the owner's setup links. Revoking
the device deletes its key and notices.

`sealed` is the envelope `1 ‖ ephemeral public key (65) ‖ nonce (12) ‖
AES-256-GCM ciphertext and tag`, base64url without padding. The AES key is
HKDF-SHA256 over the P-256 ECDH secret of the ephemeral key and the device key,
with salt `renoa.setup-notice.v1` and info `ephemeral public key ‖ device public
key`. The associated data is `renoa.setup-notice.v1 ‖ notice_id (16 bytes) ‖
device_id (16 bytes)`, so a notice opens only as itself on its device.
`crates/renoa-node/tests/setup-notice-vector.json` is a fixed vector to check an
implementation against. The plaintext is JSON:

```json
{
  "kind": "authorization",
  "label": "Notion MCP",
  "url": "https://provider.example/authorize?state=one",
  "expires_at_ms": 1893456000000
}
```

`kind` is `authorization`, a provider sign-in, or `credentials`, Renoa's
encrypted credential page; `label` names what is being connected.

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
and `false` receives only the journal. A frame without `live`, as versions
before `14` sent it, is refused.

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
and ends the command with one `assistant_message` notice and `completed`;
`/compact` followed by more words ends `failed`, and a guest's is an ordinary
message (see `rcp-v0.md`). The Pi node treats all of it as ordinary text.

Text input may also carry `"quiet": true`: nothing the command's execution
appends wakes its principal's devices, so what it has for them it asks for
itself with `request_wake`. The Renoa node submits an agent's heartbeat runs
quiet, and the coordinator makes an agent's message sent from a quiet command,
and the answer to it, quiet too. It is omitted when false.

Text input may also carry `attachments`: at most 10 files the principal
uploaded to its Host first (`PUT /v1/host/uploads/{id}`, `renoa-host-v0.md`):

```json
{
  "type": "submit",
  "request_id": 43,
  "task_id": "00000000-0000-0000-0000-000000000010",
  "command_id": "00000000-0000-0000-0000-000000000021",
  "input": {
    "type": "text",
    "text": "What can I cook with this?",
    "attachments": [
      {
        "uploadId": "00000000-0000-0000-0000-000000000030",
        "name": "fridge.jpg",
        "mimeType": "image/jpeg"
      }
    ]
  }
}
```

A name is 1 to 255 bytes with no control character, and a media type is
`type/subtype` in printable ASCII, at most 255 bytes; anything else, an unknown
field included, is `invalid_message`. RCP checks the shape only and carries the
list with the command, so a surface can show what was sent. The Renoa node gives
the agent each file (see `renoa-host-v0.md`); the Pi node fails the command
without running. It is omitted when empty.

`request_id` correlates one connection attempt. `command_id` is the durable
idempotency identity and must be reused with identical content, `context`,
`author`, `quiet` and `attachments` included, after an uncertain response.

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
  "targets": [
    { "target": "agent:alpha", "name": "Alpha" },
    { "target": "agent:arcee", "name": "Arcee" }
  ]
}
```

An invalid advertisement returns `invalid_message` with a `null` `request_id`.

A node sends a message for the agent that is executing `from_command` to
another agent of its owner:

```json
{
  "type": "send_agent_message",
  "request_id": 7,
  "from_command": "00000000-0000-0000-0000-000000000020",
  "message_id": "00000000-0000-0000-0000-000000000021",
  "node_id": "00000000-0000-0000-0000-000000000030",
  "target": "agent:arcee",
  "text": "Please summarise today's notes."
}
```

The response is `command_accepted` with the `request_id` and `message_id` as
`command_id`, or an `error` with the `request_id` and the code
[the operation](rcp-operations-v0.md#sendagentmessage) assigns. A surface that
sends it receives `invalid_role` with a `null` `request_id`.

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

A command that another agent caused carries an `origin` beside `input`, and
the same command appears in the `command_submitted` task record. The field is
absent from every other command:

```json
{
  "commandId": "00000000-0000-0000-0000-000000000021",
  "principalId": "00000000-0000-0000-0000-000000000050",
  "surface": "agent",
  "target": "agent:arcee",
  "input": { "type": "text", "text": "Please summarise today's notes." },
  "origin": {
    "fromCommand": "00000000-0000-0000-0000-000000000020",
    "from": "agent:alpha",
    "name": "Alpha",
    "depth": 1,
    "kind": "request"
  }
}
```

`kind` is `request`, `reply`, or `failure`. A surface's `submit` has no
`origin`; the coordinator sets it only for an agent message.

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

Node frames about a command use `command_id` for correlation because it is
already the durable operation identity. They do not carry `request_id`; only a
node's requests, `send_agent_message`, `request_wake` and the
[setup notice](#put-setup-notices) requests, do.

### Wake the owner

A node asks the coordinator to wake its owner's phone for something its Host
has for them that no journal record carries, such as an agent's question:

```json
{
  "type": "request_wake",
  "request_id": 8,
  "wake_id": "00000000-0000-0000-0000-000000000060",
  "urgency": "now"
}
```

`urgency` is `soon`, the same constant `changed` wake a reply's record sends,
or `now`, a `ring`. There is no quiet value: what can wait asks for no wake. The frame names no principal, task or
text; the wake goes to the node's owner. Once the wake is durably recorded, or
was already recorded for the same `wake_id` within a day, the coordinator
answers:

```json
{
  "type": "wake_requested",
  "request_id": 8
}
```

A node with no recorded owner receives `invalid_role` with the `request_id`.
A surface that sends it receives `invalid_role` with a `null` `request_id`.

### Put setup notices

A node delivers a plugin setup link for a command that did not come from
Discord by sealing it to each of its owner's active devices with a notice key.
It lists them:

```json
{
  "type": "list_notice_keys",
  "request_id": 9
}
```

```json
{
  "type": "notice_keys",
  "request_id": 9,
  "keys": [
    {
      "device_id": "00000000-0000-0000-0000-000000000080",
      "public_key": "BHq…"
    }
  ]
}
```

and puts one notice per device:

```json
{
  "type": "put_notice",
  "request_id": 10,
  "notice": {
    "notice_id": "00000000-0000-0000-0000-000000000070",
    "device_id": "00000000-0000-0000-0000-000000000080",
    "sealed": "AQS…",
    "expires_at_ms": 1893456000000
  }
}
```

```json
{
  "type": "notice_put",
  "request_id": 10
}
```

Once the notice is durably stored, with a `changed` wake for the owner, the
coordinator answers `notice_put`. A notice id names one recipient: a retry for
the same device from the same node answers `notice_put` again and stores
nothing new; one for another device or node receives `conflict` (`this notice
id names another recipient; derive a new id`). `sealed` must be 1 to 44,250
bytes of base64url without padding and `expires_at_ms` in the future, or
`invalid_message`; no error repeats the sealed text. A device that is not an
active, keyed device of the node's owner receives `invalid_role` (`the device is
not an active device of this node's owner with a notice key; list the notice
keys again`). `list_notice_keys` and `put_notice` from a node with no recorded
owner receive `invalid_role` too. When a turn stops, the node withdraws, once,
the notices of each setup link that did not reach the person; one it could not
withdraw expires with its link:

```json
{
  "type": "withdraw_notices",
  "request_id": 11,
  "notice_ids": ["00000000-0000-0000-0000-000000000070"]
}
```

```json
{
  "type": "notices_withdrawn",
  "request_id": 11
}
```

A node withdraws only notices it put; another id changes nothing. A surface
that sends a node notice frame receives `invalid_role` with a `null`
`request_id`; a node that sends a surface notice frame receives `invalid_role`
with its `request_id`.

### Host control

A node whose Host the owner controls from a browser says so once per
connection, after it authenticates:

```json
{ "type": "serve_host_control" }
```

The coordinator remembers it for that connection only; a reconnecting node says
it again. The frame has no acknowledgement, and only a node connection may send
it. The coordinator relays an authenticated owner's call, from a browser under
`/v1/host` or from an app's [`call_host`](#call-the-host), to the one connected
node of that owner that has said so:

```json
{
  "type": "host_call",
  "call_id": "00000000-0000-0000-0000-000000000090",
  "principal_id": "00000000-0000-0000-0000-000000000050",
  "method": "POST",
  "path": "/v1/host/agents",
  "body": "{\"name\":\"Scout\"}"
}
```

`method` is `GET`, `POST` or `PUT`. `principal_id` is the owner the
coordinator authenticated from the browser's session or the surface's
connection; nothing in `body` can name another. `body` is the request's JSON text, or `null` for a read. The node
decides which paths exist and what each does; the coordinator interprets
neither. The node answers on the same connection:

```json
{
  "type": "complete_host_call",
  "call_id": "00000000-0000-0000-0000-000000000090",
  "status": 200,
  "body": "{}"
}
```

The coordinator delivers an answer to the caller only from the connection the
call was sent on, and only once; any other answer is dropped. A `status` the
coordinator cannot relay (anything outside 200-299 and 400-599) becomes a
`502` for a browser and `internal` for a `call_host`. A call ends without an
answer when its connection ends or after sixty seconds. The coordinator never
retries a call and writes neither the request body nor the answer to its files.
A change survives a lost answer because the caller repeats the same request
under the same `operation_id` and the Host answers from the receipt it recorded.

### Shared files

`GET /s/{share}` serves a file an agent shared with the reader, read as either
the principal of a one-time link's `ticket` ([`share_link`](#open-a-shared-file))
or, with no `ticket` in the query, the browser's Host session; any other query
parameter is ignored. The share is the lowercase, hyphenated UUID; any other
spelling is `404`. A `HEAD` is refused with `405`, so it cannot use up a link.
With no session the answer is `401` and asks the reader to sign in; the session
cookie is `SameSite=Strict`, so a link followed from another site opens signed
out.

The coordinator relays the Host's `GET /v1/host/shares/{share}` and then each
`GET /v1/host/shares/{share}/parts/{index}` as Host control calls of the
reader's principal. It reads the description and the first part before it
answers, and only then uses up a link's ticket, so a reader who meets a Host
refusal, an unreachable Host (`503`) or a file that changed while it opened
(`409`) opens the same link again. A Host refusal, such as `404` for a share
that is gone, is the page, in the Host's words and with its status. It then
passes each part on as it arrives, keeping none of it. The answer carries the
Host's `media_type`, with `; charset=utf-8` for a `text/` type, or
`application/octet-stream` for one that is not a plain `type/subtype`; the
file's size as its `Content-Length`; the file's name in
`Content-Disposition: inline`; and `Cache-Control: private, no-store,
no-transform`. Every file but a PDF carries `Content-Security-Policy: sandbox
allow-scripts; frame-ancestors 'none'`: a shared page runs its scripts in an
opaque origin, reaches neither the session cookie nor `/v1/host`, and has no
forms, popups or dialogs with which to pass for a page of Renoa's own. It still
appears under the coordinator's address. A PDF carries `frame-ancestors 'none'`
alone, because the browser's PDF viewer refuses a sandboxed document. A later
part of the wrong size, or a last part that leaves a file whose SHA-256 is not
the one described, ends the body before that part, short of its
`Content-Length`: the reader opens the file again.

## Errors

```json
{
  "type": "error",
  "request_id": 42,
  "code": "conflict",
  "message": "command id was already used with different content"
}
```

`request_id` is `null` when no surface request can be correlated. A frame the
coordinator refuses as `invalid_message` after authentication, because it is
not a valid client frame, is answered under its `request_id` when it is a JSON
object whose `request_id` is an integer in range (see Numbers), so a surface can
drop the request it sent;
any other such frame is answered with `null`. Either way the session goes on.
Valid codes are:

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
