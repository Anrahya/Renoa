# Renoa direct MCP integration v0

## Status and authority

This document defines the first executable foundation for external Renoa
integrations: direct MCP servers over modern Streamable HTTP. The replaceable
Node process adapter implements this boundary under `adapters/mcp-client-node`.
The Host durably registers connections, publishes complete catalog snapshots,
attaches connections to Alpha's searchable registry, resolves exact references
into ordinary kernel-backed calls, and supports either no authentication or an
exact GitHub CLI account reference.

[`renoa-extensions-north-star.md`](renoa-extensions-north-star.md) owns the
broader extension direction. [`renoa-host-v0.md`](renoa-host-v0.md) owns runtime
composition. [`renoa-agent-loop-v0.md`](renoa-agent-loop-v0.md) owns the
model/tool loop. [`renoa-kernel-v0.md`](renoa-kernel-v0.md) owns durable effect
semantics. This contract narrows those designs; it does not add another tool
path or put MCP knowledge in the kernel.

The locked decisions below constrain the implementation. Exact process-wire
types and numeric limits live with the adapter code and tests. Rust types and
storage tables should be introduced only with the Host code and tests that
consume them.

## Goal

Prove this complete path:

```text
one exact Streamable HTTP endpoint
  -> discover one complete MCP tool catalog
  -> attach the connection to Alpha
  -> search compact tool summaries without schemas
  -> load only the exact schema needed now
  -> execute an exact catalog-bound reference with NeverReplay
  -> persist the exact call and result through the kernel
  -> return useful bounded content or an honest unknown outcome
```

The complete path is proved against a deterministic local server. The first
real connection uses GitHub's read-only remote MCP endpoint at
`https://api.githubcopilot.com/mcp/readonly`. Its complete accepted catalog is
searchable, but none of those schemas is sent until Alpha explicitly loads one.

## Deliberate scope

V0 supports exactly:

- MCP protocol revision `2026-07-28`;
- the modern, stateless lifecycle with per-request metadata;
- Streamable HTTP `POST` responses as either JSON or request-scoped SSE;
- `server/discover`, paginated `tools/list`, and `tools/call`;
- direct no-auth connections and one exact `gh`-resolved bearer credential;
- complete tool results containing ordered text and image blocks; and
- optional bounded structured tool output when it can be preserved without
  changing its JSON value.

It does not fall back to an `initialize` handshake or an older transport. An
incompatible server fails with a typed diagnostic. Backward compatibility can
be added later behind a new adapter revision after a real server requires it.

## Ownership

| Owner | Responsibility in this slice |
| --- | --- |
| Kernel | Persist effect intent and dispatch before invocation, freeze the runtime, settle definite results, and preserve uncertainty |
| Agent loop | Expose three fixed registry `ToolSpec`s, persist each exact `ToolCall`, and balance definite results into conversation history |
| Local Host | Own endpoint configuration, catalog refresh, profile attachment, search, exact-reference resolution, deadlines, and the adapter process lifecycle |
| MCP Node adapter | Speak the pinned MCP revision through the official SDK, validate untrusted protocol data, and report dispatch certainty |
| MCP server | Implement its tools, validate service inputs, and return protocol-compliant results |
| Surface | Present Host state and operation outcomes; never own an MCP catalog or invoke the server directly |

The kernel sees only a normal tool effect binding and its frozen revision. MCP
protocol versions, URLs, HTTP, catalogs, and server metadata remain outside it.

## Host concepts

These concepts are distinct even though the first proof uses one of each:

- **Direct integration:** one reviewed Streamable HTTP endpoint definition.
- **Connection:** one configured instance of that integration. It stores either
  no-auth or an exact `gh` hostname/account reference, never the token.
- **Catalog snapshot:** one complete, validated result of discovery and every
  `tools/list` page.
- **Profile attachment:** permission for Alpha's registry to search and resolve
  one connection. It does not advertise every tool schema.
- **Tool reference:** `mcp:<connection>:<catalog-digest>:<tool>`, returned by
  search and valid only for that exact complete catalog.
- **Resolved invocation:** the exact endpoint, protocol behavior, tool
  definition, adapter revision, and recovery class used by `tool_execute`.

Configuration does not imply discovery. Discovery does not imply profile
attachment. Attachment makes catalog entries searchable; it does not load
schemas or authorize anything beyond v0's explicit full-access profile. The
Host publishes only complete catalog snapshots.

The first durable implementation keeps these states in separate SQLite tables
under `host.sqlite3`; the SQL schema is internal Host storage, not a public
plugin or surface contract.

## Endpoint boundary

A v0 endpoint must be an absolute URL. Production endpoints use `https`.
Plain `http` is accepted only for an explicitly loopback endpoint so the real
transport can be tested without weakening remote connections.

User information and fragments are rejected. A query string is allowed but is
treated as public configuration: it is shown during inspection, contributes to
endpoint identity, and must never carry a credential. A GitHub bearer token is
resolved just in time with `gh auth token --hostname HOST --user ACCOUNT`, sent
to the adapter only through standard input, scoped to the exact configured URL,
and wiped after use. It never enters Host storage, arguments, environment,
catalog data, diagnostics, model context, or a runtime binding. Static custom
headers remain unsupported. Standard MCP headers and valid argument-derived
`x-mcp-header` values remain part of the transport contract.

Redirects are not followed. A redirect is a visible configuration failure and
the caller may review the target as a new endpoint. This keeps source identity,
request routing, and future credential scope from changing underneath an
approved connection.

The adapter uses the platform trust store. Custom certificate authorities,
client certificates, proxies, and insecure TLS switches are outside v0.

## Protocol lifecycle

The adapter pins modern MCP `2026-07-28`. Every request carries matching
protocol metadata and required Streamable HTTP headers. Client identity is
informational and stable for this adapter revision. Client capabilities are
empty because v0 offers no optional MCP feature or extension.

Discovery performs:

```text
server/discover
  -> require 2026-07-28 in supportedVersions
  -> require the tools capability
  -> tools/list until nextCursor is absent
  -> validate and normalize one complete snapshot
```

There is no `initialize`, `notifications/initialized`, `Mcp-Session-Id`, GET
stream, DELETE teardown, SSE resumption, or `Last-Event-ID`. Each request is
self-describing and independent at the MCP layer.

`server/discover` identity and instructions are untrusted presentation data.
They do not select behavior, create security identity, or enter Alpha's model
context in v0.

## Catalog discovery

Discovery and refresh are read-only Host operations. They do not run a tool,
start a kernel operation, or make any schema model-visible.

The adapter bounds every response body, SSE event, page count, cursor, tool
count, string, schema, and aggregate snapshot before publishing it. Concrete
limits live beside the process wire and its tests; changing a limit that can
alter a resolved runtime requires a new adapter binding revision.

Pagination follows these rules:

1. request pages serially in server order;
2. reject a repeated cursor or a cursor sequence that crosses the page bound;
3. reject a malformed page without publishing any part of the refresh;
4. validate every tool entry at the protocol boundary;
5. isolate and report an invalid individual entry when MCP permits that failure
   boundary, including invalid `x-mcp-header` annotations;
6. reject the entire refresh if duplicate tool names make identity ambiguous;
7. normalize accepted entries into case-sensitive bytewise tool-name order; and
8. atomically replace the prior snapshot only after all pages validate.

A failed refresh leaves the previous complete catalog untouched. It is never
merged with partial new data.

V0 refreshes only when the Host explicitly requests it. It does not open
`subscriptions/listen`, react to `notifications/tools/list_changed`, or refresh
automatically from `ttlMs`. Those mechanisms can be added once durable catalog
state and a real freshness consumer exist.

## Tool identity and model-visible naming

MCP tool names are case-sensitive. V0 accepts names from 1 through 128 ASCII
characters using letters, digits, `_`, `-`, and `.`. An invalid name is an
isolated catalog-entry failure.

Host identity is the tuple of connection identity and exact MCP tool name.
Self-reported `serverInfo.name` is never identity.

MCP names never become top-level model tool names. Alpha always sees three
small, provider-neutral Host tools:

- `tool_search` searches names, services, and descriptions and returns at most
  five compact matches with exact references, never schemas;
- `tool_load` accepts one through three unchanged references and returns their
  exact model-facing descriptions and input schemas, bounded to 64 KiB total;
- `tool_execute` accepts one unchanged reference plus an argument object and
  invokes that exact remote tool.

Search ranks deterministically and uses `*` for bounded browsing. A catalog may
hold 1,024 entries, but the model API still receives only these three fixed
schemas. Loading is explicit and atomic: an oversized group fails rather than
truncating a JSON Schema. Transport-only `x-mcp-header` annotations, titles,
icons, endpoints, protocol metadata, output schemas, and adapter bookkeeping
remain outside model context. The resolved invocation retains the unmodified
input schema so the adapter can project transport headers correctly.

## Frozen runtime identity

The runtime freezes three ordinary `AgentToolBinding`s. Search and load are
`SafeToReplay` Host reads. Execute is `NeverReplay`. Their revisions cover the
registry contract, result projection, error mapping, MCP process wire,
deadlines, and bounds. The agent-loop digest freezes their order, specs,
recovery declarations, and revisions. No kernel field or MCP-specific path is
added.

Mutable catalog data is not smuggled into those static revisions. Search reads
one current SQLite snapshot. Load and execute resolve the reference against one
current snapshot and reject a different digest as stale. The persisted
`tool_execute` request therefore carries the exact catalog identity selected by
the model; a refresh can never substitute a new schema or endpoint underneath
it.

## Invocation

Before HTTP dispatch, `tool_execute` verifies the reference syntax, Alpha
attachment, current catalog digest, exact tool name, argument-object shape, and
adapter request bound. It resolves credentials only after those local checks.
The server remains responsible for full JSON Schema and service-level
validation in v0; Renoa does not ship a partial home-grown evaluator.

The adapter sends one `tools/call` with no automatic retry. It mirrors valid
`x-mcp-header` values exactly as required by the pinned Streamable HTTP
revision. It does not add a progress token because progress notifications are
outside the first slice.

The MCP JSON-RPC request ID identifies the transport attempt. It is not an
idempotency claim. Renoa does not claim that a remote server deduplicates a
kernel `EffectId` or model tool-call ID.

## Result projection

Only a `resultType: "complete"` tool result settles as an ordinary result.
`resultType: "input_required"` is not silently retried: v0 returns a definite,
model-visible unsupported-interaction error and records that partial external
changes may be possible.

For a complete result:

- ordered MCP text blocks become ordered Renoa text blocks;
- ordered MCP image blocks become ordered Renoa image blocks without changing
  base64 data or media type;
- mixed text and image order and duplicate blocks are preserved;
- `isError: true` becomes a normal settled `ToolResult` with `is_error: true`,
  preserving actionable server content for the model;
- bounded `structuredContent` is preserved in `ToolResult.details`;
- when `outputSchema` exists, missing or SDK-invalid structured content is a
  definite invalid-result failure; and
- server-private result `_meta` is not copied into durable history or model
  context.

`ToolResult.details` preserves structured output for durable inspection but is
not provider-visible content in v0. A structured-only result with no text or
image block therefore fails as unsupported instead of returning an empty
answer to the model.

Content annotations are advisory and have no v0 policy effect. They are not
copied into Renoa's narrower `ContentBlock` type. Audio, resource links,
embedded resources, unknown content variants, invalid base64, invalid media
types, and over-limit output make the entire projection fail. No partial result
is emitted merely to make a server appear compatible.

A projection failure occurs after a server response, so the tool call itself
is known to have run. Renoa settles a definite model-visible failure with
`partial_changes_possible: true`; it does not call the tool again.

## Dispatch certainty and failures

The adapter process reports whether an HTTP request may have been dispatched.
It must publish and flush that transition before starting the HTTP request. A
missing terminal record after that transition is therefore conservative, never
falsely definite.

| Evidence | Renoa outcome |
| --- | --- |
| Endpoint, argument, encoding, process-start, or protocol validation fails before possible HTTP dispatch | Definite failure; no remote change possible |
| DNS, connection, or TLS evidence proves that no request bytes could have reached the endpoint | Definite unavailable failure |
| A valid MCP JSON-RPC error response arrives | Definite failure; partial external changes may be possible |
| A complete `tools/call` result arrives | Definite success or model-visible tool error |
| The connection, response stream, or adapter is lost after possible dispatch and before a valid terminal response | `OutcomeUnknown` |
| A timeout or cancellation occurs before possible dispatch | Definite timeout or cancellation; no remote change possible |
| A timeout or cancellation occurs after possible dispatch without a racing terminal response | `OutcomeUnknown` |

Once dispatch may have happened, a plain HTTP error without a valid MCP
terminal body is not proof that the tool did not run. It becomes unknown.

Detailed safe diagnostics retain the phase, endpoint identity, HTTP status,
JSON-RPC code, MCP request ID, adapter process status, and causal error when
available. The model receives only concise actionable tool content. Secrets,
full headers, giant bodies, and opaque server `_meta` never enter diagnostics.

## Deadlines, cancellation, and cleanup

Discovery and invocation each have a finite total deadline. The Node adapter
uses 30 seconds for discovery and 120 seconds for a call; Rust gives discovery
a 35-second outer deadline and invocation a 125-second outer deadline so
cancellation and process-group cleanup remain bounded. Invocation bounds are
part of the executable binding revision.

Progress, if added later, may extend an idle deadline but never the total
deadline. V0 has only the total deadline.

For Streamable HTTP, aborting the request closes its response stream. That asks
the server to cancel but does not prove the remote action stopped. Renoa applies
the certainty table above.

The Rust tool future does not resolve until the adapter process and any owned
local work have stopped and been reaped. If a valid terminal result races with
cancellation or child-process failure, that terminal result remains
authoritative; cleanup errors cannot replace it. If the terminal arrived but
the child does not exit, Renoa stops and reaps the child before returning the
preserved terminal.

No tool invocation retry exists inside the Host, Node adapter, HTTP client, or
official SDK. A later model-chosen call is a new logical intent with a new
durable effect, not a hidden retry.

## Adapter process boundary

The maintained MCP SDK remains behind a narrow Node process adapter under
`adapters/`. Rust owns process supervision and exposes only native Renoa
types inward.

The first process contract has two actions:

- **discover:** accept one endpoint, produce one complete normalized catalog or
  one typed terminal failure;
- **call:** accept one frozen endpoint/tool/request, report the dispatch
  transition, then produce one terminal result, definite failure, or unknown
  outcome.

The version-2 process request may carry one bounded bearer authorization value.
Standard output is a bounded machine-readable record stream. Standard error is
bounded, redacted diagnostic text and never part of the protocol. The first
valid terminal record is authoritative; later process output or cleanup failure
cannot replace it.

The exact versioned wire types and bounds live beside the adapter and Rust
process boundary and are tested at both ends; this architecture document does
not duplicate them.

## Context and observability

Discovery and search never load schemas into model context. Every normal Alpha
request carries the same three small registry specifications, independent of
whether the Host has zero, ten, or one thousand external tools. Search returns
at most five short summaries. Only a successful `tool_load` result inserts the
requested model-facing schemas into conversation history, where normal context
and compaction rules apply. Server instructions, endpoint URLs, cache hints,
output schemas, adapter bookkeeping, and every unloaded schema remain outside.

The exact `ToolCall` and settled `ToolResult` already belong to kernel-backed
semantic history. Structured `ToolResult.details` remains available for Host
inspection but is removed from every normal and compaction model request.
Runtime tracing may additionally record bounded timing and protocol
diagnostics, including discovery duration, invocation duration, dispatch
transition, HTTP status, and terminal classification. It must not duplicate raw
secret-bearing payloads.

Surfaces observe existing agent tool lifecycle events and the final durable
outcome. MCP does not create a second UI event authority.

## Refresh and recovery

Catalog refresh is safe to request again because it is read-only. Each attempt
builds a fresh snapshot and atomically publishes it or changes nothing. V0 does
not automatically retry one attempt; the caller may issue another explicit
refresh.

Every registry call opens current Host state instead of consulting a process
cache. A connection attached or refreshed by a GUI, another local command, or
the running agent becomes visible to the next `tool_search` call, including
inside an already active Alpha turn. Waku, ACP, and Alpha do not restart. This
does not alter an in-flight remote call. The stateless v0 MCP adapter also does
not keep a subscription open for `notifications/tools/list_changed`; a Host
refresh must publish that remote change first.

Tool invocation is never replayed after possible dispatch. The kernel's
existing `NeverReplay` recovery turns an interrupted dispatched effect into
`OutcomeUnknown` without invoking the MCP adapter again. An explicitly
abandoned unknown tool operation uses the existing balanced-history behavior;
MCP adds no special transcript rule.

A refresh does not mutate the three frozen registry implementations. A search
or load read that must be replayed may observe later committed registry state,
like any safe external read. A not-yet-dispatched execute with an old digest
fails stale; a possibly dispatched execute is never called again. Renoa never
substitutes the newer catalog.

## Proven full Host path

The complete vertical path must prove, through a deterministic local MCP server
and the real process boundary:

1. pinned modern discovery succeeds without `initialize` or session headers;
2. an older-only server fails without fallback;
3. pagination, deterministic ordering, duplicate names, cursor loops, malformed
   pages, and atomic refresh behave exactly as specified;
4. JSON and SSE responses produce the same terminal result;
5. valid `x-mcp-header` routing works and invalid declarations are isolated;
6. ordered text/image content and duplicate blocks survive unchanged;
7. unsupported, malformed, and over-limit results fail atomically;
8. `isError` content reaches the model as a settled tool result;
9. pre-dispatch failures are definite while post-dispatch loss is unknown;
10. no layer retries one `tools/call`;
11. cancellation and deadlines stop and reap local work without claiming the
    remote action stopped;
12. a terminal result wins races with cancellation, stderr overflow, nonzero
    exit, and hung cleanup;
13. diagnostics are bounded and redact sensitive header and URL forms;
14. every model request advertises only the three registry schemas, while
    search over 1,000 entries returns no schema and load returns only requested
    exact schemas;
15. one live registry object sees a committed attachment without restart, and
    catalog replacement makes prior references fail stale;
16. restart never repeats a possibly dispatched tool call;
17. an exact `gh` account reference resolves a token only at invocation, while
    adapter output, diagnostics, Host SQLite, and frozen bindings remain
    secret-free; and
18. durable structured details remain Host-visible but never reach a normal or
    compaction model request.

## Locked decisions

- MCP is one replaceable tool adapter, not a kernel, loop, RCP, or surface
  protocol.
- The first revision is modern MCP `2026-07-28` over Streamable HTTP only.
- Connections are direct and use either no auth or one exact `gh` CLI account
  reference; Renoa stores no GitHub token.
- Discovery publishes only complete, bounded, deterministic catalog snapshots.
- The stored Host identity is composite; self-reported server names are not
  identity.
- Alpha exposes three fixed registry tools rather than every external schema.
- Search and load are `SafeToReplay`; exact remote execution is `NeverReplay`.
- An exact catalog digest in every reference prevents silent schema changes.
- Committed Host changes are visible on the next registry call without an
  agent or surface restart.
- No invocation layer performs an automatic retry.
- Only explicitly loaded exact schemas become model-visible.
- Existing kernel effect certainty, cancellation, and recovery semantics remain
  authoritative.

## Open decisions after this slice

- pre-2026 MCP compatibility and stdio transport;
- Renoa-owned OAuth/API-key flows, general secret storage, and configured
  headers;
- future Host schema migrations and typed management commands;
- automatic refresh, cache hints, and list-change subscriptions;
- progress projection;
- standards-complete client-side argument validation and any schema behavior
  beyond the pinned SDK's structured-output validation;
- MCP resources, prompts, apps, tasks, and multi-round-trip input;
- a safe invocation retry policy, only after service-specific idempotency and
  reconciliation can prove its duplicate semantics;
- process pooling or multiplexing;
- tool approval and permission policy; and
- package loading through Agent Plugins.

## Evidence

Reviewed on 2026-08-27. This contract copies no upstream source.

- [MCP specification `2026-07-28` at `5f5440bb26a62e2cf3440b92da5a667efa03b267`](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/5f5440bb26a62e2cf3440b92da5a667efa03b267), with the repository's Apache-2.0 transition, remaining MIT material, and CC-BY-4.0 documentation.
- [MCP TypeScript SDK 2.0.0 at `cc4b41617ce3601b1290d67216ea0b194a3cd9ac`](https://github.com/modelcontextprotocol/typescript-sdk/tree/cc4b41617ce3601b1290d67216ea0b194a3cd9ac). The published `@modelcontextprotocol/client@2.0.0` package declares MIT; the source repository records the broader MCP license transition.
- [GitHub MCP server at `a00dc319edcb5f8a10f118b1dad649c94928aac4`](https://github.com/github/github-mcp-server/tree/a00dc319edcb5f8a10f118b1dad649c94928aac4), MIT. Renoa copied no server source; the reviewed endpoint and read-only tool catalog are consumed through MCP.
- [OpenAI Agents SDK at `10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e`](https://github.com/openai/openai-agents-python/tree/10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e), MIT. Its deferred tool loading and namespaces were reviewed; no source was copied.
- [DeepSeek Harness at `b150a551b8d465e31e418e1b2eaf5e79bbb7d28e`](https://github.com/deepseek-ai/deepseek-harness/tree/b150a551b8d465e31e418e1b2eaf5e79bbb7d28e), MIT. Its single code-mode transport informed the constant-schema direction; no source was copied.
- [Anthropic Tool Search documentation](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool). Its deferred-definition behavior and measured large-catalog context cost were reviewed; no source was copied.

The SDK is an implementation dependency behind Renoa's adapter process, not
Renoa's internal domain model or public Rust API.
