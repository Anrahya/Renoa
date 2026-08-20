# ACP v1 adapter

## Status

The first local coding-frontend adapter is implemented. It uses stable Agent
Client Protocol wire version 1 over newline-delimited JSON-RPC on standard I/O.
The Rust SDK package is `agent-client-protocol@2.0.0`; that package version is
not ACP wire version 2. Unstable ACP features are disabled.

ACP is not part of the harness and is not RCP. The dependency direction is:

```text
coding frontend -> ACP adapter -> durable Renoa harness -> model and local tools
```

The harness remains usable without ACP. RCP can later sit outside the harness
and provide cross-device task continuity without changing this frontend
contract.

## Process contract

The frontend starts:

```sh
renoa-agent acp
```

`renoa-agent --version` prints the version without starting the transport. ACP
messages use standard input and output. Diagnostics use standard error so they
cannot corrupt JSON-RPC framing. One process owns one active ACP session.

The ACP adapter still uses the legacy harness backend. Until it is migrated to
the kernel-backed Alpha Host path, that process reads:

- `RENOA_PI_BRIDGE`
- `RENOA_PI_PROVIDER`
- `RENOA_PI_MODEL`
- `RENOA_PI_AUTH_STORE`
- `RENOA_PI_INSTRUCTIONS`
- optional `RENOA_DATA_DIR`

Without `RENOA_DATA_DIR`, sessions use Renoa's platform data directory.
`RENOA_PI_PROVIDER` selects the provider hosted by this process.
`RENOA_PI_MODEL` is the initial model for a new session; it is not a fixed UI
choice. Authentication remains local to the provider adapter.
`RENOA_PI_INSTRUCTIONS` is legacy ACP configuration; the kernel-backed local
runner uses Alpha's versioned prompt and does not read it.

## Implemented ACP behavior

- `initialize` negotiates stable protocol version 1.
- `session/new` creates a durable standalone harness session.
- `session/load` reopens that session after the ACP process exits.
- `session/new` and `session/load` return standard ACP `model` and
  `thought_level` select options.
- `session/set_config_option` changes the model or reasoning level and returns
  the complete, updated option set.
- `session/prompt` accepts text, image, and resource-link blocks.
- `session/cancel` durably cancels active model or tool work and stops its child
  process before returning `cancelled`.
- Model text and reasoning deltas stream while inference is running.
- Tool start, progress, completion, failure, and final assistant text stream as
  ACP session updates.
- The final prompt response is sent only after the harness has durably settled
  the operation.

The adapter advertises only `loadSession` and image prompts. Audio, embedded
resources, additional workspace directories, and MCP servers are rejected.

The model list comes from Pi's authenticated provider catalog. Renoa validates
and pins the selected model specification before building a runtime, so a
newly discovered model is not fetched again between selection and use.
Reasoning choices come from that model's declared capability map. A model
change keeps the current reasoning level when the new model supports it;
otherwise it uses `high`, or the model's first supported level. Configuration
changes are rejected while a prompt is active, so one operation cannot switch
provider behavior midway through execution.

## Durable identity and resume

Each session is stored at:

```text
<data-directory>/sessions/<session-uuid>/session.json
<data-directory>/sessions/<session-uuid>/runtime.jsonl
<data-directory>/sessions/<session-uuid>/harness.sqlite3
```

The manifest binds the session UUID to one canonical workspace path. The
manifest, initial runtime selection, and harness session all exist before
`session/new` is acknowledged.
`session/load` requires the same UUID and canonical workspace, then reconstructs
model context from the durable harness transcript.

`runtime.jsonl` is an append-only record of acknowledged model and reasoning
changes. Each complete record is synced before the ACP response. Reload uses
the last complete record and ignores an incomplete crash tail. The provider's
current validated model binding is included in the harness runtime revision, so
recovery cannot silently execute pending work under different model behavior.

T3 Code sends one UUID in `_meta.requestId` and `_meta.promptId` for each turn.
Renoa reuses that UUID as the harness request identity. A redelivered settled
prompt therefore returns its existing durable outcome without another model
call. If both fields are present, they must match. Clients that omit both fields
receive a generated identity and do not get lost-request idempotency across
processes.

ACP v1 does not replay old transcript notifications during `session/load`. The
frontend retains its presentation history; the harness retains the
authoritative execution transcript needed to continue the agent correctly.

## Current limits

- Local standard-I/O transport only; no draft ACP v2 remote transport.
- One session and one active prompt per process.
- All locally configured tools run without approval prompts. Permission policy
  remains a future harness/product feature, not an ACP rule.
- No MCP servers, mode switching, account methods, or extra workspace roots are
  advertised. SuperGrok login is still performed before launch rather than
  through ACP account methods.
- ACP live updates are transient. Missed cross-device replay belongs to RCP,
  not this adapter.
- If the client loses the successful `session/new` response, stable ACP v1
  provides no caller-supplied creation identity or session-list operation for
  recovering that unknown UUID. The durable session can be orphaned. Turn
  admission does not have this limitation because T3 supplies stable turn IDs.
