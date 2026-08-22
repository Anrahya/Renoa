# ACP v1 adapter

## Status

The first local coding-frontend adapter is implemented. It uses stable Agent
Client Protocol wire version 1 over newline-delimited JSON-RPC on standard I/O.
The Rust SDK package is `agent-client-protocol@2.0.0`; that package version is
not ACP wire version 2. Unstable ACP features are disabled.

ACP is a surface adapter, not part of the kernel and not RCP. The dependency
direction is:

```text
coding frontend -> ACP adapter -> Renoa local Host -> kernel
                                              |       |
                                              |       `-> durable agent loop
                                              `----------> Pi model and local tools
```

The Host and kernel remain usable without ACP. RCP can later provide
cross-device task continuity without changing this frontend contract.

## Process contract

The frontend starts:

```sh
renoa-agent acp
```

`renoa-agent --version` prints the version without starting the transport. ACP
messages use standard input and output. Diagnostics use standard error so they
cannot corrupt JSON-RPC framing. One process owns one active ACP session.

The process reads:

- `RENOA_PI_BRIDGE`
- `RENOA_PI_PROVIDER`
- `RENOA_PI_MODEL`
- `RENOA_PI_AUTH_STORE`
- optional `RENOA_DATA_DIR`

Without `RENOA_DATA_DIR`, sessions use Renoa's platform data directory.
`RENOA_PI_PROVIDER` selects the provider hosted by this process.
`RENOA_PI_MODEL` is the initial model for a new session; it is not a fixed UI
choice. Authentication remains local to the provider adapter.
The adapter always resolves Renoa Alpha v1, including its curated base prompt
and bounded workspace `AGENTS.md` instructions. An environment variable cannot
replace Alpha's instructions. The Host reads `AGENTS.md` again before each new
turn and freezes the result only when the kernel admits that operation.

## Implemented ACP behavior

- `initialize` negotiates stable protocol version 1.
- `session/new` creates a durable Alpha Agent and kernel Session.
- `session/load` reopens that session after the ACP process exits.
- `session/load` replays the complete kernel-backed transcript before its
  response, using durable kernel event UUIDs as ACP message IDs.
- `session/close` cancels active work, waits for adapter cleanup, and releases
  the process for another session.
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
- The final prompt response is sent only after the kernel has durably settled
  the operation.

Transient model and tool events are routed directly from the agent-loop
adapters to ACP. This observer is excluded from the frozen runtime manifest and
authoritative history. The kernel remains surface-blind; completed output is
projected from durable semantic events before ACP returns success.

The adapter advertises `loadSession`, `session/close`, and image prompts. Audio,
embedded resources, additional workspace directories, and MCP servers are
rejected.

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
<data-directory>/sessions/<session-uuid>/kernel.sqlite3
<data-directory>/sessions/<session-uuid>/trace.sqlite3
```

The versioned manifest binds the Alpha profile, Agent identity, Session
identity, and canonical workspace. The Host builds the manifest, initial
runtime selection, and kernel database in one hidden staging directory, syncs
them, then atomically publishes the directory under the session UUID before
`session/new` is acknowledged.
`session/load` requires the same UUID and canonical workspace, then reconstructs
model context from gapless kernel semantic history. It also verifies that the
manifest's Agent identity matches the kernel's durable Session binding.

`runtime.jsonl` is an append-only record of acknowledged model and reasoning
changes. Each complete record is synced before the ACP response. Reload uses
the last complete record and truncates an incomplete crash tail before another
record can be appended. The provider's current validated model binding is
included in the kernel runtime manifest, so
recovery cannot silently execute pending work under different model behavior.

`trace.sqlite3` is the separate diagnostic timeline. One run records ordered
wall-clock timestamps and elapsed times, model time-to-first-output and total
duration, every stream chunk, exact provider-neutral and translated provider
requests, redacted response headers, normalized input/output/cache tokens, and
tool inputs, progress, results, durations, and typed failures. It is never read
to rebuild model context or decide kernel recovery.

An ACP client can send one UUID in `_meta.requestId` and `_meta.promptId` for
each turn. Renoa reuses that UUID as the kernel command identity. A redelivered
settled prompt therefore returns its existing durable outcome without another
model call. If both fields are present, they must match. Clients that omit both
fields receive a generated identity and do not get lost-request idempotency
across processes.

If a process stopped after admitting a turn but before settling it, a different
new turn is rejected before admission or model execution. Retrying the original
turn identity and exact content resumes that durable operation.

If model or tool execution has an outcome that cannot be proven, the local Host
explicitly abandons that operation without replay. ACP returns the honest
failure, while the repaired loop history and released session allow a later
turn to continue.

During `session/load`, Renoa validates gapless semantic history and sends its
user, assistant, reasoning, tool-call, and tool-result updates before the load
response. A surface may keep a presentation cache, but it must reconcile that
cache with the replayed durable event identities instead of treating a second
transcript as execution truth.

## Current limits

- Local standard-I/O transport only; no draft ACP v2 remote transport.
- One session and one active prompt per process.
- All locally configured tools run without approval prompts. Permission policy
  remains a future Host/product feature, not an ACP rule.
- No MCP servers, mode switching, account methods, or extra workspace roots are
  advertised. SuperGrok login is still performed before launch rather than
  through ACP account methods.
- An active turn's live deltas are transient. Reload reconstructs settled local
  history; cross-device delivery continuity still belongs to RCP.
- Earlier pre-release session manifests used storage versions 1 and 2. This
  adapter rejects them explicitly instead of guessing at an execution or trace
  migration.
- If the client loses the successful `session/new` response, stable ACP v1
  provides no caller-supplied creation identity or session-list operation for
  recovering that unknown UUID. The durable session can be orphaned. Turn
  admission does not have this limitation because T3 supplies stable turn IDs.
