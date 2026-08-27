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
                                              `----------> model adapter and local tools
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

Before creating a session, a surface can discover the authenticated model and
reasoning choices with:

```sh
renoa-agent models --json
```

This read-only command uses the same provider settings as ACP, marks the
configured initial model and each model's default reasoning level, and does not
create or modify durable session state.

The product CLI can install Renoa's first read-only GitHub connection with the
same Host data root and adapter configuration:

```sh
renoa-agent mcp github install --account ACCOUNT
```

This resolves the exact account through `gh`, refreshes the complete remote
catalog, and attaches that connection to Alpha's searchable registry. It stores
the hostname/account reference, not the token. No GitHub schema is advertised
until Alpha loads an exact search result.

The process reads:

- `RENOA_MODEL_BRIDGE`
- optional `RENOA_MODEL_PROVIDERS`
- `RENOA_MODEL_PROVIDER`
- `RENOA_MODEL`
- `RENOA_MODEL_AUTH_STORE`
- optional `RENOA_DATA_DIR`
- optional `RENOA_MCP_ADAPTER`

Without `RENOA_DATA_DIR`, Host state uses Renoa's platform data directory.
`RENOA_MCP_ADAPTER` is the absolute path to the built MCP process adapter. It
enables Host catalog refresh and invocation. A tool reaches Alpha only after a
Host profile attachment such as the GitHub command above. A committed change is
visible on the next registry call without restarting ACP or the surface.
`RENOA_MODEL_PROVIDERS` is a comma-separated enabled set; when absent, it
defaults to the single `RENOA_MODEL_PROVIDER`. `RENOA_MODEL_PROVIDER` and
`RENOA_MODEL` select the default provider and raw model ID for a new session.
Surfaces see collision-safe `provider/model` choices and can switch both with
the standard ACP model selector. Renoa durably stores provider and model as
separate fields, so `session/load` restores the exact adapter rather than the
current process default. Authentication remains local to the provider adapter,
and every explicitly enabled provider must have a usable credential.
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
- `session/delete` permanently removes one closed session. Repeating deletion
  is safe; deleting a session still owned by a process is rejected.
- `session/new` and `session/load` return standard ACP `model` and
  `thought_level` select options.
- `session/new` and `session/load` advertise `compact` through the standard
  `available_commands_update` session update.
- `session/set_config_option` changes the model or reasoning level and returns
  the complete, updated option set.
- `session/prompt` accepts text, image, and resource-link blocks.
- A sole text block equal to `/compact` after outer whitespace is a control
  operation. Arguments or attachments are rejected; similarly named text such
  as `/compactly` remains an ordinary prompt.
- `session/cancel` durably cancels active model or tool work and stops its child
  process before returning `cancelled`.
- Model text and reasoning deltas stream while inference is running.
- Tool start, progress, completion, failure, and final assistant text stream as
  ACP session updates.
- Every provider call with reported token usage emits the standard ACP
  `usage_update`. `used` is that call's input, cache-read, cache-write, and
  output tokens; `size` is the active model's validated context window.
- A successful `/compact` emits only the durable post-compaction
  `usage_update`, then returns `end_turn` with
  `_meta.renoa.controlResult = "compact"`. A surface can present that typed
  result as status instead of inventing an assistant reply. The internal
  summary stream is not presented as an assistant message.
- `session/load` emits the newest durable provider usage or post-compaction
  estimate after replaying the transcript, so a restarted surface restores the
  meter without another model call.
- The final prompt response is sent only after the kernel has durably settled
  the operation.

Transient model and tool events are routed directly from the agent-loop
adapters to ACP. This observer is excluded from the frozen runtime manifest and
authoritative history. The kernel remains surface-blind; completed output is
projected from durable semantic events before ACP returns success.

The adapter advertises `loadSession`, `session/close`, `session/delete`, and
image prompts. Audio, embedded resources, additional workspace directories,
and surface-supplied MCP server declarations are rejected.

The model list comes from the Renoa-owned provider adapter's authenticated,
pinned catalog. Renoa validates and pins the selected model specification
before building a runtime, so a newly discovered model is not fetched again
between selection and use.
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
settled prompt or compact control therefore returns its existing durable
outcome without another model call. If both fields are present, they must
match. Clients that omit both fields receive a generated identity and do not
get lost-request idempotency across processes.

The command identity remains separate from ACP message identity. Live
assistant messages receive one agent-generated `messageId` per message.
Replayed message chunks keep their durable semantic-event `messageId`, while a
replayed user chunk carries its originating command UUID in `_meta.requestId`.
That lets a surface reconcile an optimistic user entry without treating a
client correlation value as the agent-owned message identity.

If a process stopped after admitting a turn but before settling it, a different
new turn is rejected before admission or model execution. Retrying the original
turn identity and exact content resumes that durable operation.

Process recovery follows the frozen effect policy; it does not promise that an
ambiguous external call ran exactly once. A settled effect is never repeated.
An interrupted safe model, read, grep, or find effect may be dispatched again
with its exact durable identity and request. An interrupted edit, write, or Bash
effect is never dispatched again: Renoa settles the operation as failed and
persists a model-visible result explaining that the tool may have finished but
its result is unknown.

A crash-resuming surface must durably retain the Renoa Session UUID and the
exact unresolved turn UUID and content before sending that turn. After restart,
it loads the exact Session, reconciles its presentation cache from Renoa's
complete replay, and only then redelivers the same unresolved turn. It must not
substitute current UI configuration, infer execution state from cached text, or
silently create another Session when load fails.

If recovery reaches an explicit unknown outcome after applying that policy, the
local Host abandons the operation without another dispatch. The durable effect
stays `OutcomeUnknown`. ACP still captures the redacted `ModelRequestFailed`
event as a `session_info_update` and returns that concise provider error as the
terminal JSON-RPC error, so the UI can show what failed while kernel truth
remains unknown.

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
- No surface-supplied MCP servers, mode switching, account methods, or extra
  workspace roots are advertised. MCP catalogs stay Host-owned; Alpha receives
  only the fixed search/load/execute registry tools. SuperGrok login is still
  performed before launch rather than through ACP account methods.
- An active turn's live deltas are transient. Reload reconstructs settled local
  history; cross-device delivery continuity still belongs to RCP.
- Earlier pre-release session manifests used storage versions 1 and 2. This
  adapter rejects them explicitly instead of guessing at an execution or trace
  migration.
- Agent-loop revision 9 and checkpoint schema 3 are forward-only for unfinished
  operations. A revision-8 operation needs its original runtime to finish; the
  current Host does not migrate frozen manifests. An older binary also cannot
  decode the new compact control command.
- If the client loses the successful `session/new` response, stable ACP v1
  provides no caller-supplied creation identity or session-list operation for
  recovering that unknown UUID. The durable session can be orphaned. Turn
  admission does not have this limitation because T3 supplies stable turn IDs.
