# Renoa direct MCP integration v0

## Status and authority

This document defines the first executable foundation for external Renoa
integrations: direct MCP servers over modern Streamable HTTP. The replaceable
Node process adapter implements this boundary under `adapters/mcp-client-node`.
The Host durably registers connections, publishes complete catalog snapshots,
attaches connections to exact agent profiles, resolves exact references
into ordinary kernel-backed calls, and supports no authentication, an exact
GitHub CLI account reference, a named Secret Service bearer or header
reference, or a Host-owned OAuth 2.1 browser flow using Client ID Metadata Documents,
pre-registered client credentials, or Dynamic Client Registration.
Desktop Hosts use a loopback callback and Secret Service. Headless Hosts use
the same OAuth state machine with a short-lived callback relay at the Renoa
control origin and an owner-only local secret directory.

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
  -> attach the connection to one registered profile
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

The first Renoa-owned service is the Agent Plugin package at
`plugins/google-drive`. Its stateless Streamable HTTP Worker at
`https://drive.renoa.live/mcp` stores no credential: the Host obtains and
refreshes Google OAuth, supplies the access token for one request, and the
Worker scopes it only to Google's Drive API origins. Google Drive API v3
currently exposes 64 raw REST methods; Renoa deliberately publishes eight
task-shaped tools: recent files, search, metadata, text read, bounded byte
download, create, copy, and permission read. The package belongs to the
Host-wide inventory and profiles opt into it, so no surface owns Drive logic.

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
  Secret Service credentials projected into a validated header and prefix;
- Host-owned OAuth 2.1 authorization-code flows with PKCE for remote HTTP MCP
  servers, all three standard client registration approaches, issuer-bound
  credentials, and automatic just-in-time refresh;
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
  credential reference plus its validated header and public prefix. OAuth
  connections also store the selected registration
  policy: a CIMD URL, a named pre-registered client reference, or explicit DCR.
  They store only a deterministic token-bundle reference, durable non-secret
  flow phase, and semantic terminal receipt in SQLite, never a token or client
  secret.
- **Catalog snapshot:** one complete, validated result of discovery and every
  `tools/list` page.
- **Profile attachment:** permission for one exact registered profile to search
  and resolve one connection. It does not advertise every tool schema.
- **Tool reference:** `mcp:<connection>:<catalog-digest>:<tool>`, returned by
  search and valid only for that exact complete catalog.
- **Resolved invocation:** the exact endpoint, protocol behavior, tool
  definition, adapter revision, and recovery class used by `tool_execute`.

Configuration does not imply discovery. Discovery does not imply profile
attachment. Attachment makes catalog entries searchable; it does not load
schemas or authorize anything beyond v0's explicit full-access profile. The
Host publishes only complete catalog snapshots.

Connections and catalog snapshots are one Host-wide inventory. Attachment rows
are profile-scoped: the same exact connection may be attached to any number of
registered profiles without copying its catalog, while an attachment to one
profile grants no visibility to another.

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
Schema v9 adds the explicit OAuth registration policy. Existing v8 OAuth
connections migrate to DCR because that is the only behavior the v8 runtime
implemented. Their connection identity, catalog, profile attachment, flow, and
receipt records remain intact.
Schema v10 adds an exact header name and public prefix for generic Secret
Service credentials. Existing no-auth, GitHub CLI, bearer, and OAuth rows
migrate without changing identity or catalog state.
Schema v11 adds only the private shared Agent Plugin registry identity and
cursor. It does not change MCP integrations, connections, credentials,
catalogs, attachments, tool identity, or execution.
Schema v12 adds a mutually exclusive callback-relay identity beside the
existing loopback port. Existing flows migrate with their exact loopback port;
no credential, catalog, attachment, or receipt changes owner.
Schema v13 removes the active-connection foreign key from OAuth attempts and
receipts. This lets a proposed connection finish authorization before its
configuration is published; existing OAuth state migrates unchanged.
Catalogs produced by released adapter revisions v0.1, v0.2, v0.4, v0.5, v0.6,
v0.7, v0.8, and v0.9 remain readable by the v0.10 Host. The two early revisions retain their
original headerless digest encoding, so their durable references remain exact
rather than being silently rewritten during an upgrade. New discovery always
publishes the current revision.

## Endpoint boundary

A v0 endpoint must be an absolute URL. Production endpoints use `https`.
Plain `http` is accepted only for an explicitly loopback endpoint so the real
transport can be tested without weakening remote connections.

User information and fragments are rejected. A query string is allowed but is
treated as public configuration: it is shown during inspection, contributes to
endpoint identity, and must never carry a credential. A GitHub bearer token is
resolved just in time with `gh auth token --hostname HOST --user ACCOUNT`. A
named credential is resolved just in time. A desktop Host uses `secret-tool
lookup application renoa credential ID`; a configured headless Host uses its
owner-only private credential directory. The Host sends its secret plus the connection's reviewed
header name and public prefix to the adapter only through standard input. The
adapter forms that exact header and scopes it to the configured URL. Rust wipes
its owned secret buffers after use.
It never enters arguments, environment, catalog data, diagnostics, model
context, or a runtime binding. Only the selected Host's secret facility stores
the value.

An OAuth credential bundle is bound to the exact configured MCP endpoint at
both the Node and Rust process boundaries. Its Secret Service reference is
derived from the connection and endpoint identity. This prevents a token from
being reused for another service even if separate local data stores reuse a
connection name.

A pre-registered client uses a separate named Host credential containing
strict JSON with `schema_version`, `issuer`, `client_id`, and an optional
`client_secret`. Desktop stores use Secret Service; headless stores use the
same private credential directory as named API tokens. The issuer is required because OAuth client identifiers are
authorization-server-specific. The Host resolves this item just in time and
passes it only over adapter standard input. CIMD URLs and DCR policy are public
configuration; client IDs, client secrets, and tokens are not model arguments.

An integration may supply bounded fixed public headers, such as Exa's source
identifier. Renoa rejects authorization, API-key, cookie, MCP, content, and
other client-owned header names so package data cannot impersonate a credential
or change transport control. The separately configured Secret Service header
may use a credential header such as `authorization`, `x-api-key`, or `cookie`,
but cannot replace transport-owned headers or collide with public package
headers. Standard MCP headers, the Host-resolved credential, and valid
argument-derived `x-mcp-header` values remain authoritative at the request
boundary.

Redirects are not followed. A redirect is a visible configuration failure and
the caller may review the target as a new endpoint. This keeps source identity,
request routing, and future credential scope from changing underneath an
approved connection.

The adapter uses the platform trust store. Custom certificate authorities,
client certificates, proxies, and insecure TLS switches are outside v0.

## OAuth lifecycle

OAuth remains Host connection policy, not MCP tool behavior and not kernel
state. `extension_manage` can add or connect a package with `credential.kind =
"oauth"`; this is the model's only OAuth choice. Before producing any setup or
sign-in link, the Host requires RFC 9728 protected-resource metadata naming
exactly one authorization server and valid metadata for that exact issuer. It
then reuses a saved issuer-bound client, uses the Renoa HTTPS Client ID Metadata
Document when advertised, uses DCR when advertised, or requests a developer-app
client bound to the discovered issuer. The model cannot choose a mode, issuer,
or credential label. The Host validates a proposed connection without publishing
it, then authenticates and discovers its complete catalog.
Only one final SQLite transaction publishes the connection, catalog, and
profile attachment. A failed new connection leaves none of those rows; a failed
replacement leaves the prior working configuration untouched. `authorize` resumes an existing
flow. `restart: true` is the explicit instruction to abandon an expired or
unknown flow and discard cached tokens before starting again.

Initial authorization follows MCP scope selection: use the exact scope from the
initial `WWW-Authenticate` challenge when present; otherwise use the protected
resource's advertised `scopes_supported`; otherwise omit `scope`. Renoa does
not guess provider-specific names. If a later call returns HTTP 403
`insufficient_scope`, the adapter returns one definite failure with the exact
validated `required_scope` and does not retry the call. The agent may pass that
exact value to `authorize` for an existing connection, or to a repeated
`connect` for an unpublished connection. The Host unions it with the prior
grant and opens fresh consent when the grant must widen. Only after
authorization succeeds may the agent explicitly retry the original call once.
This keeps write intent and its retry visible to the kernel.

The Host creates a cryptographically random state value and one exact callback
identity before asking the pinned MCP client SDK to perform protected-resource
and authorization-server discovery. A desktop Host binds `127.0.0.1` on an
ephemeral port and opens the authorization URL with `xdg-open` as an argument,
never through a shell. A headless Host reserves a ten-minute callback at the
configured Renoa HTTPS origin and emits the provider authorization URL as
structured surface progress. The person can open that link on another device;
the provider redirects to `/v1/oauth/callback` and the Host continues polling.

The callback relay is an auxiliary Host-auth service, not RCP task data. Relay
management requires a separately enrolled Node device credential. The public
provider callback is located only by the SHA-256 digest of the 256-bit OAuth
state. The relay durably records one bounded code or rejection before replying
to the browser. Exact duplicate callbacks are idempotent; conflicting reuse is
rejected. The Host writes the result to its local secret facility before its
idempotent acknowledgement clears the relay copy. A crash or network failure
therefore resumes from either durable side without repeating an OAuth
credential POST. Relay HTTP delivery may retry only transient transport or
gateway failure with the same client-selected relay identity; MCP and OAuth
credential operations keep their no-hidden-retry rule. The relay never receives
the PKCE verifier, client secret, access token, or refresh token.

Missing API-token and Host-selected pre-registered OAuth-client references on a configured
headless Host use a separate short-lived credential-intake relay. The Host
first writes a random AES-256-GCM key, capability, relay identity, and operation
binding into an owner-only local file. It sends only the capability digest and
non-secret form metadata to the coordinator. The surface receives a permanent
HTTPS action whose URL fragment contains the key, capability, and the
Host-discovered OAuth issuer when applicable; URL fragments
are not sent in HTTP requests. The browser fetches a same-origin, strictly
content-security-policy-bound page, encrypts the typed credential locally with
a fresh 96-bit nonce and relay-bound additional data, then submits only the
nonce, ciphertext, and capability. The coordinator validates the capability,
durably stores one idempotent ciphertext, and cannot decrypt it.

The Host polls with its enrolled Node credential, decrypts and validates the
typed value, rejects an OAuth client for any issuer other than the one bound to
the setup link, atomically stores it in its private credential directory, and only
then acknowledges the relay. Acknowledgement clears the coordinator's nonce
and ciphertext. Process restart reuses the locally retained relay identity,
key, and capability; it never creates a second secret prompt for the same tool
operation. The credential-intake relay is not a credential-sharing service:
the resulting value belongs only to the requesting Host.

The control origin publicly serves Renoa's non-secret Client ID Metadata
Document with a `client_id` equal to that document's exact URL and its exact
callback URI. The Host supplies that document only when
the authorization server advertises CIMD. A saved pre-registered client takes
priority; otherwise CIMD takes priority over advertised DCR. If neither is
available, the Host requests the provider's developer-app client. Both callback
modes validate the state and optional `iss` value before exchange.

Persisted dynamically registered clients and tokens are returned only for the
same validated authorization-server issuer that produced them. Pre-registered
credentials are rejected when discovery resolves a different issuer. A changed
authorization server therefore cannot inherit another server's client or token.

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
context, Host SQLite, environment variables, or package content. Desktop Hosts
store OAuth state and tokens in Secret Service. A configured headless Host uses
atomic files inside a `0700` directory with `0600` entries; the callback-relay
device credential comes from an owner-only or service-manager credential file.
Packages are portable. Connections and credentials remain local to the Host;
this callback transport does not implement cross-node credential sharing.

An existing connection remains immutable by default. `replace: true` on
`add` or `connect` is the explicit repair path for a wrong endpoint or
credential policy. The replacement candidate is authenticated and discovered
while the old connection stays authoritative. One Host transaction then swaps
the connection, catalog, and attachment and clears obsolete OAuth state.
Repeating the same replacement is a no-op and preserves its
catalog. A successful unauthenticated discovery reports `catalog_loaded`; only
a completed OAuth flow reports `authorized`.

`extension_manage disconnect` is narrower than replacement or removal. It
deletes only the active profile's attachment in one transaction and is idempotent.
The durable connection, catalog, package, credential reference, OAuth state,
and receipts remain available for recovery and later reattachment. List output
therefore reports registration, authentication kind, catalog presence, and
profile attachment as separate facts; it never infers that an OAuth token is
currently valid merely because the connection is registered.
`extension_manage enable` is the symmetric, idempotent reattachment path. It
requires the retained complete catalog and performs no network request.
Management list output also reports the committed accepted and rejected skill
bindings per plugin source, separately from immutable package installation.
It flattens those states into compact pages of at most 32 facts. The opaque
continuation cursor fingerprints the complete inventory, so a concurrent Host
change invalidates that cursor instead of making offset pagination skip or
repeat an entry.
The management tool advertises one flat model-facing schema because several
OpenAI-compatible providers drop arguments when a function schema is rooted in
`oneOf`. Its action description states the required companion fields, while the
Host decodes one closed typed variant per action. Missing fields and fields from
another action are rejected rather than ignored.

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

MCP names never become top-level model tool names. Every assembled profile sees
three small, provider-neutral Host tools:

- `tool_search` searches names, services, and descriptions and returns at most
  200 compact matches containing only names, descriptions, and exact
  references, never schemas;
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

Before HTTP dispatch, `tool_execute` verifies the reference syntax,
active-profile attachment, current catalog digest, exact tool name,
argument-object shape, and adapter request bound. During discovery the Node
adapter compiles every input schema with the pinned MCP SDK's AJV-backed validator and isolates an invalid
tool definition. At invocation it recompiles the frozen catalog schema and
validates the exact arguments before declaring dispatch. The server remains
responsible for service-level validation; Renoa does not ship a partial
home-grown schema evaluator. Credential resolution follows the local Host
checks, and schema failure remains before possible HTTP dispatch.

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
- when `outputSchema` exists, a successful result with missing or SDK-invalid
  structured content is a definite invalid-result failure; an `isError: true`
  result may omit success-only structured content so its diagnostic text still
  reaches the model; and
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

The process contract has seven actions:

- **discover:** accept one endpoint, produce one complete normalized catalog or
  one typed terminal failure;
- **call:** accept one frozen endpoint/tool/request, report the dispatch
  transition, then produce one terminal result, definite failure, or typed
  uncertain failure;
- **oauth_discover:** require protected-resource metadata, resolve exactly one
  authorization-server issuer, and report only the client modes that its
  metadata advertises;
- **oauth_begin:** discover OAuth metadata and produce either an authorization
  redirect, an existing usable credential, or one typed failure;
- **oauth_exchange:** exchange exactly one saved callback code;
- **oauth_token:** inspect saved token state without network mutation; and
- **oauth_refresh:** perform exactly one refresh attempt.

The version-9 process request may carry one bounded exact credential header
name, public prefix, and secret value plus bounded fixed public headers. OAuth
begin, exchange, and refresh requests carry one exact registration object.
OAuth begin may also carry one validated scope copied from a prior
`oauth_insufficient_scope` result. A pre-registered object includes its
issuer and resolved client fields only for the lifetime of that adapter
process. Local `oauth_token` inspection carries no registration credential.
The version-9 call wire can return that exact challenged scope in a typed
diagnostic. It is also part of the frozen `tool_execute` binding, so an
unfinished version-7 execution cannot resume under changed process semantics.
Standard output is a bounded machine-readable record stream. Standard error is
bounded, redacted diagnostic text and never part of the protocol. The first
valid terminal record is authoritative; later process output or cleanup failure
cannot replace it.

The exact versioned wire types and bounds live beside the adapter and Rust
process boundary and are tested at both ends; this architecture document does
not duplicate them.

## Context and observability

Discovery and search never load schemas into model context. Every normal profile
request carries the same three small registry specifications, independent of
whether the Host has zero, ten, or one thousand external tools. Search returns
at most 200 short summaries. Only a successful `tool_load` result inserts the
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
inside an already active Agent turn. The surface and Agent do not restart. This
does not alter an in-flight remote call. The stateless v0 MCP adapter also does
not keep a subscription open for `notifications/tools/list_changed`; a Host
refresh must publish that remote change first.

Tool invocation is never replayed after possible dispatch. The kernel's
existing `NeverReplay` recovery turns an interrupted dispatched effect into
`OutcomeUnknown` without invoking the MCP adapter again. During a live call,
the Host converts a typed no-response outcome into a durable, model-visible
uncertain tool result so the Agent can continue reasoning without replay. A
process crash before that result is persisted still follows the kernel's
conservative recovery boundary.

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
17. an exact `gh` account or named Secret Service reference resolves a secret
    only at invocation, while adapter output, diagnostics, Host SQLite, and
    frozen bindings remain secret-free;
18. an Exa-shaped package sends its reviewed public source header and
    just-in-time bearer through the real adapter, and a live registry sees the
    attached catalog without restart;
19. durable structured details remain Host-visible but never reach a normal or
    compaction model request;
20. a cancelled browser flow resumes against the exact saved callback without
    repeating registration, while SQLite contains no code, token, or state;
21. concurrent expired-token reads perform one rotating refresh and a lost
    refresh becomes durable unknown rather than being replayed; and
22. explicit reauthorization drops cached tokens, endpoint-bound state cannot
    cross services, callback state is exact, and provider failures are bounded
    and redacted;
23. replay of the same settled OAuth management operation reads its terminal
    receipt without a second browser flow or credential POST;
24. strict OAuth preflight accepts exactly one metadata-backed issuer, then
    selects an existing issuer client, advertised CIMD, advertised DCR, or an
    issuer-bound developer client without a model-chosen fallback;
25. a pre-registered client skips DCR, authenticates the token exchange, never
    crosses to a different issuer, and never appears in adapter output;
26. v8 OAuth connections migrate to explicit DCR without losing durable Host
    state, and explicit replacement drops stale tools but is idempotent; and
27. one corrupt installed package is reported separately without hiding valid
    packages from `extension_manage list`; and
28. disconnect immediately removes search and execution access, survives
    replay, and retains the exact complete catalog for later reattachment;
29. enable reattaches that retained catalog without network access, and list
    reports package integrity, connection state, and plugin skill bindings as
    separate facts;
30. a generic Secret Service credential reaches the exact configured header
    with its public prefix, while collisions, malformed names, and secret leaks
    fail at the boundary; and
31. invalid discovered schemas are isolated, and required, enum, maximum, and
    additional-property violations fail against the frozen input schema before
    any remote dispatch; and
32. management inventory pagination returns every compact fact in order, while
    a package, connection, or skill change invalidates an earlier cursor;
33. a headless Host resumes a relayed callback after interruption, persists the
    code locally before clearing it remotely, and handles provider rejection by
    the same durable acknowledgement rule; and
34. the relay admits only enrolled Node credentials, survives coordinator
    restart, rejects conflicting callback reuse, and never stores PKCE or token
    material; and
35. encrypted browser intake reaches the requesting Host over the real relay,
    while failed new connections publish no configuration and failed
    replacements preserve the previous working connection; and
36. the credential form serializes the frozen `oauth_client` wire name while
    accepting the earlier `o_auth_client` spelling only during rolling
    deployment; and
37. initial OAuth follows the MCP challenge/metadata scope order, while one
    insufficient-scope response returns its exact grant, sends one remote call,
    and requires an explicit authorization and retry; and
38. the headless OAuth-client form displays a read-only discovered issuer,
    binds it into authenticated encryption, and rejects any different issuer.

## Locked decisions

- MCP is one replaceable tool adapter, not a kernel, loop, RCP, or surface
  protocol.
- The first revision prefers modern MCP `2026-07-28` and accepts only the
  pinned SDK's enumerated legacy revisions over Streamable HTTP.
- Connections are direct and use no auth, one exact `gh` CLI account reference,
  one named Secret Service credential with an exact header and prefix, or
  Host-owned OAuth; Renoa stores no secret in SQLite or package data.
- OAuth uses PKCE, one exact loopback or self-hosted relay callback,
  endpoint-bound local secret state, Host-selected client registration policy,
  authorization-server issuer binding, MCP-selected initial scopes, explicit
  scope accumulation, explicit durable phases, one credential POST per adapter
  operation, no hidden tool retry, and no automatic replay after an uncertain
  exchange.
- Authorization-server quirks remain replaceable adapter policy. For Google's
  issuer, authorization requests require offline access and fresh consent so a
  one-hour access token does not publish a connection that cannot be refreshed.
  Extension-manager revision v16 owns that changed authorization request.
  Execution-registry revision v2 turns a pre-dispatch OAuth failure into a
  settled, model-visible error while recording that the MCP tool never ran. An
  unfinished operation frozen to either earlier revision cannot resume under
  the new behavior; settled operations remain replayable.
- The callback relay stores only a short-lived code or rejection after hashing
  state, clears that result only after the execution Host has persisted it, and
  never becomes a credential-sharing service or an RCP task record.
- Headless credential intake sends the coordinator only form metadata,
  capability hashes, and browser-encrypted bytes. The decryption key remains
  with the browser link and execution Host; connection publication waits for
  credential validation and authenticated discovery.
- Fixed integration headers are public, bounded data. Sensitive and
  client-owned header names are rejected.
- Discovery publishes only complete, bounded, deterministic catalog snapshots.
- The stored Host identity is composite; self-reported server names are not
  identity.
- Every assembled profile exposes three fixed registry tools rather than every
  external schema.
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
- credential revocation, cross-platform desktop secret stores, and cross-node
  secret synchronization;
- future Host schema migrations beyond v13;
- catalog cache hints and list-change subscriptions;
- progress projection;
- schema dialects or custom keywords not accepted by the pinned SDK validator;
- MCP resources, prompts, apps, tasks, and multi-round-trip input;
- a safe invocation retry policy, only after service-specific idempotency and
  reconciliation can prove its duplicate semantics;
- process pooling or multiplexing;
- tool approval and permission policy; and
- stdio or SSE Agent Plugin MCP entries.

## Evidence

Reviewed on 2026-09-05. This contract copies no upstream source. The remote
callback design follows OAuth 2.0 Security Best Current Practice (RFC 9700),
OAuth 2.0 for Native Apps (RFC 8252), and MCP authorization revision
`2026-07-28`; Renoa still owns its relay and durability implementation.

- [MCP specification `2026-07-28` at `5f5440bb26a62e2cf3440b92da5a667efa03b267`](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/5f5440bb26a62e2cf3440b92da5a667efa03b267), with the repository's Apache-2.0 transition, remaining MIT material, and CC-BY-4.0 documentation.
- [MCP TypeScript SDK 2.0.0 at `cc4b41617ce3601b1290d67216ea0b194a3cd9ac`](https://github.com/modelcontextprotocol/typescript-sdk/tree/cc4b41617ce3601b1290d67216ea0b194a3cd9ac). The published `@modelcontextprotocol/client@2.0.0` package declares MIT; the source repository records the broader MCP license transition.
- [GitHub MCP server at `a00dc319edcb5f8a10f118b1dad649c94928aac4`](https://github.com/github/github-mcp-server/tree/a00dc319edcb5f8a10f118b1dad649c94928aac4), MIT. Renoa copied no server source; the reviewed endpoint and read-only tool catalog are consumed through MCP.
- [OpenAI Agents SDK at `10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e`](https://github.com/openai/openai-agents-python/tree/10cdae4a3c30a29c6e96c8ec14e6bf1c5f02940e), MIT. Its deferred tool loading and namespaces were reviewed; no source was copied.
- [OpenCode v2 at `dc4449df0d52199704ea4989a5a993ebbc605612`](https://github.com/anomalyco/opencode/tree/dc4449df0d52199704ea4989a5a993ebbc605612), MIT. Its discriminated local/remote MCP configuration, explicit lifecycle status, and connect/disconnect controls informed Renoa's exact management actions; Renoa keeps credentials in Secret Service and copied no source.
- [Pi at `853a80d26c90a14c1886f0ebb8ffaae133ca2185`](https://github.com/badlogic/pi-mono/tree/853a80d26c90a14c1886f0ebb8ffaae133ca2185), MIT. Its exact TypeBox tool contracts and runtime tool registration were reviewed; Renoa retained its frozen durable bindings and copied no source.
- [DeepSeek Harness at `cd5ef8148158c3a752a658978873241fdf8e2bbc`](https://github.com/deepseek-ai/deepseek-harness/tree/cd5ef8148158c3a752a658978873241fdf8e2bbc), MIT. Its atomic MCP generation replacement, failed-refresh retention, and bounded reconnect behavior informed Renoa's enable and hot-load lifecycle; no source was copied.
- [Anthropic Tool Search documentation](https://platform.claude.com/docs/en/agents-and-tools/tool-use/tool-search-tool). Its deferred-definition behavior and measured large-catalog context cost were reviewed; no source was copied.
- [Agent Plugins 1.0 at `ff8ab5e392cc87bd88d87c060815a87490e51003`](https://github.com/agentplugins/agent-plugins-spec/tree/ff8ab5e392cc87bd88d87c060815a87490e51003), with CC-BY-4.0 specification text and Apache-2.0 schemas. Renoa consumes its package and MCP shapes without copying runtime source.
- [Exa MCP server at `15ffb50519e719dc791cdc750ce5ed1934c0a1ed`](https://github.com/exa-labs/exa-mcp-server/tree/15ffb50519e719dc791cdc750ce5ed1934c0a1ed), MIT. Renoa copied no server source; its Agent Plugin endpoint, public source header, and bearer boundary form the first real package-shaped proof.
- [Google Drive API v3 REST reference](https://developers.google.com/workspace/drive/api/reference/rest/v3). Renoa implements eight focused operations over the stable Drive REST API instead of mirroring its full raw method catalog.
- [Cloudflare stateless remote MCP guide](https://developers.cloudflare.com/agents/model-context-protocol/guides/remote-mcp-server/). The first-party Drive server uses the current stateless `createMcpHandler` boundary and stores no session or credential state.

The SDK is an implementation dependency behind Renoa's adapter process, not
Renoa's internal domain model or public Rust API.
