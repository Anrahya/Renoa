# Renoa direct MCP integration v0

## Status and authority

This document defines the first executable foundation for external Renoa
integrations: direct MCP servers over modern Streamable HTTP. The replaceable
Node process adapter implements this boundary under `adapters/mcp-client-node`.
The Host durably registers connections, publishes complete catalog snapshots,
attaches connections to Alpha's searchable registry, resolves exact references
into ordinary kernel-backed calls, and supports no authentication, an exact
GitHub CLI account reference, a named Secret Service bearer reference, or a
Host-owned OAuth 2.1 browser flow.

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

- preferred MCP protocol revision `2026-07-28` plus the exact legacy revisions
  supported by the pinned SDK (`2025-11-25`, `2025-06-18`, `2025-03-26`,
  `2024-11-05`, and `2024-10-07`);
- the modern stateless lifecycle and the SDK's legacy initialize/session
  lifecycle, selected during discovery and then frozen in the catalog;
- Streamable HTTP `POST` responses as either JSON or request-scoped SSE;
- `server/discover`, paginated `tools/list`, and `tools/call`;
- direct no-auth connections, exact `gh`-resolved bearer credentials, and named
  Secret Service bearer credentials;
- Host-owned OAuth 2.1 authorization-code flows with PKCE for remote HTTP MCP
  servers, followed by automatic just-in-time refresh;
- bounded public request headers supplied by a reviewed integration or Agent
  Plugin package;
- complete tool results containing ordered text and image blocks; and
- optional bounded structured tool output when it can be preserved without
  changing its JSON value.

Discovery prefers modern negotiation and may fall back once to the SDK's
legacy initialize handshake. A stored catalog call uses only that exact
negotiated revision; it neither repeats discovery nor changes protocol. An
unsupported server fails with a typed diagnostic.

## Ownership

| Owner | Responsibility in this slice |
| --- | --- |
| Kernel | Persist effect intent and dispatch before invocation, freeze the runtime, settle definite results, and preserve uncertainty |
| Agent loop | Expose three fixed registry `ToolSpec`s, persist each exact `ToolCall`, and balance definite results into conversation history |
| Local Host | Own endpoint configuration, catalog refresh, profile attachment, search, exact-reference resolution, OAuth coordination, deadlines, and adapter process lifecycles |
| MCP Node adapter | Speak the pinned MCP revision through the official SDK, validate untrusted protocol data, and report dispatch certainty |
| MCP server | Implement its tools, validate service inputs, and return protocol-compliant results |
| Surface | Present Host state and operation outcomes; never own an MCP catalog or invoke the server directly |

The kernel sees only a normal tool effect binding and its frozen revision. MCP
protocol versions, URLs, HTTP, catalogs, and server metadata remain outside it.

## Host concepts

These concepts are distinct even though the first proof uses one of each:

- **Direct integration:** one reviewed Streamable HTTP endpoint definition.
- **Connection:** one configured instance of that integration. It stores
  no-auth, an exact `gh` hostname/account reference, or a named Secret Service
  credential reference. OAuth connections store only a deterministic secret
  reference, durable non-secret flow phase, and semantic terminal receipt in
  SQLite, never a token.
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

Upgrade behavior is explicit: schema v1 and v2 stored individual Alpha tool
selections. V3 converts any selection from a connection into one connection
attachment, making that connection's complete current catalog available under
Alpha's deliberate full-access v0 policy. This is an intentional widening for
the current profile, not a future permission rule; a later permission model
must replace it rather than silently inheriting it.

Host schema v4 adds the separate Agent Skills records. Schema v6 adds immutable
Agent Plugin records, bounded public MCP headers, and named credential
references. It stores no credential value. These additions do not change MCP
tool identity, attachment, catalog-reference, or execution behavior; the
v1/v2-to-v3 MCP migration remains the same proven transformation.
Schema v7 preserves package homepage metadata and adds a plugin skill scope;
neither changes the MCP wire or tool identity.
Schema v8 adds the OAuth connection kind, durable flow phases, and terminal
operation receipts. A receipt is keyed by stable session, command, and
tool-call identity and contains only the semantic outcome needed to replay the
same management effect without opening another browser or repeating an OAuth
POST. OAuth client state, PKCE verifiers, authorization codes, access tokens,
refresh tokens, client secrets, and remote failure text remain outside SQLite.

## Endpoint boundary

A v0 endpoint must be an absolute URL. Production endpoints use `https`.
Plain `http` is accepted only for an explicitly loopback endpoint so the real
transport can be tested without weakening remote connections.

User information and fragments are rejected. A query string is allowed but is
treated as public configuration: it is shown during inspection, contributes to
endpoint identity, and must never carry a credential. A GitHub bearer token is
resolved just in time with `gh auth token --hostname HOST --user ACCOUNT`. A
named API key is resolved just in time with `secret-tool lookup application
renoa credential ID`. The Host sends either value to the adapter only through
standard input, scopes it to the exact configured URL, and wipes it after use.
It never enters Host storage, arguments, environment, catalog data,
diagnostics, model context, or a runtime binding.

An OAuth credential bundle is bound to the exact configured MCP endpoint at
both the Node and Rust process boundaries. Its Secret Service reference is
derived from the connection and endpoint identity. This prevents a token from
being reused for another service even if separate local data stores reuse a
connection name.

An integration may supply bounded fixed public headers, such as Exa's source
identifier. Renoa rejects authorization, API-key, cookie, MCP, content, and
other client-owned header names so package data cannot impersonate a credential
or change transport control. Standard MCP headers, the Host-resolved bearer,
and valid argument-derived `x-mcp-header` values remain authoritative at the
request boundary.

Redirects are not followed. A redirect is a visible configuration failure and
the caller may review the target as a new endpoint. This keeps source identity,
request routing, and future credential scope from changing underneath an
approved connection.

The adapter uses the platform trust store. Custom certificate authorities,
client certificates, proxies, and insecure TLS switches are outside v0.

## OAuth lifecycle

OAuth remains Host connection policy, not MCP tool behavior and not kernel
state. `extension_manage` can add or connect a package with `credential.kind =
"oauth"`; the Host registers the connection before authentication but publishes
and attaches its catalog only after authorization and authenticated discovery
both succeed. `authorize` resumes an existing flow. `restart: true` is the
explicit instruction to abandon an expired or unknown flow and discard cached
tokens before starting again.

The Host binds an exact `127.0.0.1` callback on an ephemeral port, creates a
cryptographically random state value, persists the callback identity and phase,
then asks the pinned MCP client SDK to perform protected-resource and
authorization-server discovery. The SDK uses PKCE and either advertised client
metadata or Dynamic Client Registration. Renoa opens the authorization URL with
`xdg-open` as an argument, never through a shell. The callback accepts one
bounded GET from loopback, requires the exact Host header and state, records the
code in Secret Service before acknowledging the browser, and validates `iss`
when the server advertises it. The callback expires after ten minutes.

Credential-side POST requests are never retried inside one adapter operation.
The Host records `begin`, code exchange, and refresh as explicit durable phases.
After process loss, a committed terminal credential state may be reconciled; an
exchange that may have reached the authorization server without a durable
terminal becomes `unknown` and is never repeated automatically. Access-token
inspection is local. Expired tokens refresh under a process-crash-safe file
lock, so concurrent sessions share one rotating refresh-token exchange. A
revoked but unexpired token produces the server's ordinary model-visible 401;
the agent can then invoke explicit reauthorization.

`extension_manage` is safe to replay only because the Host also commits a
bounded terminal receipt before returning. Re-entry with the same stable
session/command/tool-call identity reads that receipt and performs no remote
OAuth mutation. An authorized receipt is accepted only while its endpoint-bound
credential still resolves locally; otherwise the operation fails closed and a
new command must explicitly authorize again. Definite remote failure receipts
record only their class, not the server message or diagnostics. Receipts remain
until their connection is removed; garbage collection after kernel settlement
is deferred until the Host has a proven settlement boundary.

Authorization URLs may be emitted as surface progress while the browser is
waiting. Raw OAuth state and credentials never enter tool arguments, model
context, Host SQLite, environment variables, or package content. V0 requires a
desktop Secret Service (`secret-tool`) and a browser opener on the executing
node. Packages are portable; connections and credentials remain node-local.

## Protocol lifecycle

The adapter prefers modern MCP `2026-07-28`. Every modern request carries
matching protocol metadata and required Streamable HTTP headers. Client
identity is informational and stable for this adapter revision. Client
capabilities are empty because v0 offers no optional MCP feature or extension.

Discovery performs:

```text
server/discover
  -> use 2026-07-28 when offered
  -> otherwise perform one SDK legacy initialize/initialized exchange
  -> require the tools capability
  -> tools/list until nextCursor is absent
  -> validate and normalize one complete snapshot
```

Modern operation has no protocol session. Legacy operation permits only the
SDK's exact initialize, initialized notification, negotiated session header,
single event stream, and action-appropriate `tools/list` or `tools/call`
sequence. The guarded transport rejects an invented or changed session ID,
unexpected method, hidden `tools/call` retry, redirect, or second event stream.
The negotiated revision is durable catalog data and every call request carries
it back to the adapter.

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
| An HTTP error response arrives, with or without a valid MCP terminal body | Definite model-visible failure containing the safe status and server message; partial external changes may be possible |
| A complete `tools/call` result arrives | Definite success or model-visible tool error |
| The connection, response stream, or adapter is lost after possible dispatch and before a valid terminal response | Model-visible uncertain result stating that the call may or may not have succeeded |
| A timeout or cancellation occurs before possible dispatch | Definite timeout or cancellation; no remote change possible |
| A timeout or cancellation occurs after possible dispatch without a racing terminal response | Model-visible uncertain result stating that the call may or may not have succeeded |

An HTTP response is evidence that the server answered, so its safe status and
message reach the model instead of being replaced by a generic lost-result
error. `partial_changes_possible` remains true after dispatch because an error
response does not prove that the server made no external change.

If no terminal response arrives after dispatch, the Host settles an honest
error result that says the call may or may not have succeeded. This keeps the
model loop alive without pretending success or failure. No Renoa layer retries
the call; the model must explain the uncertainty or verify state safely before
deciding whether a new call is appropriate.

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

The process contract has six actions:

- **discover:** accept one endpoint, produce one complete normalized catalog or
  one typed terminal failure;
- **call:** accept one frozen endpoint/tool/request, report the dispatch
  transition, then produce one terminal result, definite failure, or typed
  uncertain failure;
- **oauth_begin:** discover OAuth metadata and produce either an authorization
  redirect, an existing usable credential, or one typed failure;
- **oauth_exchange:** exchange exactly one saved callback code;
- **oauth_token:** inspect saved token state without network mutation; and
- **oauth_refresh:** perform exactly one refresh attempt.

The version-5 process request may carry one bounded bearer authorization value
and bounded fixed public headers.
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
`OutcomeUnknown` without invoking the MCP adapter again. During a live call,
the Host converts a typed no-response outcome into a durable, model-visible
uncertain tool result so Alpha can continue reasoning without replay. A process
crash before that result is persisted still follows the kernel's conservative
recovery boundary.

A refresh does not mutate the three frozen registry implementations. A search
or load read that must be replayed may observe later committed registry state,
like any safe external read. A not-yet-dispatched execute with an old digest
fails stale; a possibly dispatched execute is never called again. Renoa never
substitutes the newer catalog.

## Proven full Host path

The complete vertical path must prove, through a deterministic local MCP server
and the real process boundary:

1. preferred modern discovery succeeds without `initialize` or session headers;
2. an older-only server falls back once, stores the exact negotiated revision,
   and calls through that revision without modern rediscovery;
3. pagination, deterministic ordering, duplicate names, cursor loops, malformed
   pages, and atomic refresh behave exactly as specified;
4. JSON and SSE responses produce the same terminal result;
5. valid `x-mcp-header` routing works and invalid declarations are isolated;
6. ordered text/image content and duplicate blocks survive unchanged;
7. unsupported, malformed, and over-limit results fail atomically;
8. `isError` content reaches the model as a settled tool result;
9. received HTTP failures preserve their safe status and server message, while
   post-dispatch loss becomes an honest model-visible uncertainty;
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
17. an exact `gh` account or named Secret Service reference resolves a token
    only at invocation, while adapter output, diagnostics, Host SQLite, and
    frozen bindings remain secret-free;
18. an Exa-shaped package sends its reviewed public source header and
    just-in-time bearer through the real adapter, and a live registry sees the
    attached catalog without restart; and
19. durable structured details remain Host-visible but never reach a normal or
    compaction model request;
20. a cancelled browser flow resumes against the exact saved callback without
    repeating registration, while SQLite contains no code, token, or state;
21. concurrent expired-token reads perform one rotating refresh and a lost
    refresh becomes durable unknown rather than being replayed; and
22. explicit reauthorization drops cached tokens, endpoint-bound state cannot
    cross services, callback state is exact, and provider failures are bounded
    and redacted; and
23. replay of the same settled OAuth management operation reads its terminal
    receipt without a second browser flow or credential POST.

## Locked decisions

- MCP is one replaceable tool adapter, not a kernel, loop, RCP, or surface
  protocol.
- The first revision prefers modern MCP `2026-07-28` and accepts only the
  pinned SDK's enumerated legacy revisions over Streamable HTTP.
- Connections are direct and use no auth, one exact `gh` CLI account reference,
  one named Secret Service bearer reference, or Host-owned OAuth; Renoa stores
  no token in SQLite or package data.
- OAuth uses PKCE, exact loopback callbacks, endpoint-bound Secret Service
  state, explicit durable phases, one credential POST per adapter operation,
  and no automatic replay after an uncertain exchange.
- Fixed integration headers are public, bounded data. Sensitive and
  client-owned header names are rejected.
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

- compatibility outside the enumerated MCP revisions and stdio transport;
- headless/device authorization, GUI credential entry, credential revocation,
  cross-platform secret stores, and cross-node secret synchronization;
- future Host schema migrations beyond v8;
- catalog cache hints and list-change subscriptions;
- progress projection;
- standards-complete client-side argument validation and any schema behavior
  beyond the pinned SDK's structured-output validation;
- MCP resources, prompts, apps, tasks, and multi-round-trip input;
- a safe invocation retry policy, only after service-specific idempotency and
  reconciliation can prove its duplicate semantics;
- process pooling or multiplexing;
- tool approval and permission policy; and
- stdio or SSE Agent Plugin MCP entries.

## Evidence

Reviewed on 2026-08-29. This contract copies no upstream source.

- [MCP specification `2026-07-28` at `5f5440bb26a62e2cf3440b92da5a667efa03b267`](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/5f5440bb26a62e2cf3440b92da5a667efa03b267), with the repository's Apache-2.0 transition, remaining MIT material, and CC-BY-4.0 documentation.
- [MCP TypeScript SDK 2.0.0 at `cc4b41617ce3601b1290d67216ea0b194a3cd9ac`](https://github.com/modelcontextprotocol/typescript-sdk/tree/cc4b41617ce3601b1290d67216ea0b194a3cd9ac). The published `@modelcontextprotocol/client@2.0.0` package declares MIT; the source repository records the broader MCP license transition.
- [GitHub MCP server at `a00dc319edcb5f8a10f118b1dad649c94928aac4`](https://github.com/github/github-mcp-server/tree/a00dc319edcb5f8a10f118b1dad649c94928aac4), MIT. Renoa copied no server source; the reviewed endpoint and read-only tool catalog are consumed through MCP.
- [OpenAI Agents SDK at `10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e`](https://github.com/openai/openai-agents-python/tree/10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e), MIT. Its deferred tool loading and namespaces were reviewed; no source was copied.
- [DeepSeek Harness at `b150a551b8d465e31e418e1b2eaf5e79bbb7d28e`](https://github.com/deepseek-ai/deepseek-harness/tree/b150a551b8d465e31e418e1b2eaf5e79bbb7d28e), MIT. Its single code-mode transport informed the constant-schema direction; no source was copied.
- [Anthropic Tool Search documentation](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool). Its deferred-definition behavior and measured large-catalog context cost were reviewed; no source was copied.
- [Agent Plugins 1.0 at `ff8ab5e392cc87bd88d87c060815a87490e51003`](https://github.com/agentplugins/agent-plugins-spec/tree/ff8ab5e392cc87bd88d87c060815a87490e51003), with CC-BY-4.0 specification text and Apache-2.0 schemas. Renoa consumes its package and MCP shapes without copying runtime source.
- [Exa MCP server at `15ffb50519e719dc791cdc750ce5ed1934c0a1ed`](https://github.com/exa-labs/exa-mcp-server/tree/15ffb50519e719dc791cdc750ce5ed1934c0a1ed), MIT. Renoa copied no server source; its Agent Plugin endpoint, public source header, and bearer boundary form the first real package-shaped proof.

The SDK is an implementation dependency behind Renoa's adapter process, not
Renoa's internal domain model or public Rust API.
