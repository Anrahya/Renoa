# Renoa local Host v0

## Purpose

The Renoa Host is the trusted composition layer between surfaces and the
durable kernel. It resolves one durable Agent Instance into the exact runtime
that can execute its next operation.

The Host is not another agent loop and it does not replace the kernel:

```text
surface
   |
   | command and observation
   v
Renoa Host
   |-- resolve model, loop, context, skills, and tools
   |-- supervise the local runtime
   `-- offer one exact Runtime to the kernel
             |
             v
        Renoa Kernel
   durable admission, effects, events, and recovery
```

`renoa-local` is the first Host implementation. Product surfaces are maintained
outside this core repository and connect through ACP. The current graphical
surface is the separate Renoa integration fork of Waku. ACP is the standard
agent-facing surface protocol. Renoa-specific capability management will use a
separate logical Host API whose transport is not selected until a real
management consumer is implemented.

The first concrete agent profile is Renoa Alpha v1, specified in
[`renoa-alpha-v1.md`](renoa-alpha-v1.md). Its stable Host identity is
`renoa.coding.alpha.v1`. Alpha is one built-in profile, not a special Host
execution type. A Host process registers one or more `AgentProfile` recipes and
can create a session from any exact registered `AgentProfileId`.

The product direction for portable packages, external integrations,
connections, profile selection, and agent-driven capability changes is recorded
in [`renoa-extensions-north-star.md`](renoa-extensions-north-star.md). That
document does not override this Host boundary or prematurely settle the open
v0 storage, permission, and management contracts below.

The first direct external-tool boundary is specified in
[`renoa-mcp-v0.md`](renoa-mcp-v0.md). It resolves MCP tools through the existing
Host and `AgentToolBinding` path rather than adding a parallel runtime.

## What lives where

The same component has three distinct states:

```text
Capability library   installed pieces available to the Host
Agent profile        a recipe selecting desired pieces and configuration
Resolved runtime     exact bound pieces for one kernel operation
```

The Host owns the capability library, profiles, runtime resolution, provider
configuration, credentials, workspace bindings, and future policy. The kernel
owns Agent and Session identity, command admission, operation state, semantic
events, effect safety, checkpoints, and the frozen `RuntimeManifest`.

The kernel stores compatibility identities and revisions for selected pieces.
It does not store or interpret provider, tool, skill, workspace, or product
configuration.

## Agent identity and assembly

An Agent Instance is durable identity and isolated history. It is not the
temporary collection of Rust objects used to execute one operation.

For each operation, the Host conceptually resolves:

```text
Agent Instance + profile + installed capabilities + current scope
                              |
                              v
                    exact resolved Runtime
```

The kernel freezes that runtime before execution. A later profile or component
change cannot replace its implementations. The fixed extension registry is an
explicit exception for data visibility: it may read later committed Host state,
but every executable reference is catalog-bound and fails stale rather than
changing behavior.

Profiles are declarative recipes. They do not execute effects and do not own
session history. Installed availability does not imply future authorization;
that distinction remains required even though v0 intentionally has no
permission system.

V0 profiles are immutable process configuration: a stable validated identity,
base instructions, and whether the canonical workspace-root `AGENTS.md` is
included. A session manifest persists the selected profile identity beside its
Agent, Session, and workspace identity. Loading fails closed when that exact
profile is not registered by the current Host process. Profile definitions are
not yet editable or stored in `host.sqlite3`.

The current Host creates one Agent Instance and its first Session together. A
durable Agent catalog and multiple Sessions per Agent remain future work.
Telegram, WhatsApp, ACP, a GitHub webhook, and a GUI are surfaces or ingress
adapters; they do not become profiles merely because they deliver messages. A
GitHub-review recipe or a daily-assistant recipe is a profile and may be used
from any compatible surface.

## Full-access first slice

Permission semantics are deliberately open. V0 does not introduce roles,
levels, grants, approval records, or a permission trait.

Every currently registered local profile is all-allowed. Every tool registered
by its local workspace provider is advertised directly. External catalogs are
reached through three fixed registry tools so catalog size does not become
model context. The current top-level set is:

```text
read_file
edit_file
write_file
bash
grep
find
tool_search
tool_load
tool_execute
extension_manage
skill_search
skill_load
```

Existing tool invariants remain in force. File tools stay within the configured
workspace. Bash starts in that workspace but is unrestricted and is not a
sandbox for untrusted work. "Full access" means no new Host-level filtering; it
does not mean weakening existing adapter correctness or cancellation behavior.

Model-visible output is bounded. Reads use one-based pagination; Bash preserves
the final output and stops its process group after 120 seconds by default. A
call may choose 1 through 1,800 seconds; timeout results include retained output
and warn that partial changes may already exist. Every other built-in tool has
a 120-second Host deadline. A deadline or cancellation is not reported until
the adapter has stopped its owned work. Grep and find return
deterministic workspace-relative results and explicit truncation notices.
Search delegates regex traversal and ignore semantics to the resolved `rg`
executable, then applies positive path globs without overriding ignored files.
Grep and find skip hidden paths, including `.git`; unrestricted Bash is the
explicit path for hidden-file access. Its reported revision is part of each
search binding identity.

`write_file` and `edit_file` commit through a same-directory temporary file,
sync it, atomically rename it, and sync the parent directory. Existing file
permissions are preserved. `edit_file` also rechecks the content it read before
rename, so a concurrent change becomes a typed conflict instead of a lost
update. A failure after rename is outcome-unknown rather than a false claim
that nothing changed.

When a real permission consumer is designed, effective capabilities will be
resolved before runtime construction. Forbidden tools must then be absent from
the model request and independently rejected by their execution boundary. No
permission-shaped fields are reserved in this slice.

## Current concrete composition

`LocalHost` owns the provider configuration, durable data root, Agent Plugin
library, MCP catalog, skill library, and credential resolution boundary.
`host.sqlite3` keeps installed package metadata, supported package MCP entries,
direct integration and connection identities, non-secret credential references,
durable non-secret OAuth phases and terminal receipts, complete MCP catalog
snapshots, per-profile attached connection identities, immutable skill revisions,
source/profile bindings, rejected skill entries, and session activation pins.
Registration, discovery, and profile attachment remain separate states.
Catalog replacement and attachment are transactional, and multi-query reads use
one SQLite snapshot so a registry call cannot observe half of a refresh.

An optional private shared plugin registry replicates only the immutable Agent
Plugin library between Hosts. Each Host remains a complete local runtime and
keeps its own credentials, MCP connections and discovered catalogs, profile
attachments, session skill activations, workspaces, and session history. The
registry is a separate Host service, not the RCP coordinator and not a remote
kernel. Its stable UUID is the authority identity; its URL is only a replaceable
route. A Host binds to one identity and fails closed if an endpoint later names
a different registry.

Package upload is an idempotent content-addressed `PUT`. The service fully
writes, hashes, and syncs an archive before committing its next SQLite revision
and acknowledging it. The ordered change feed is read from one SQLite snapshot.
A receiving Host downloads the exact length and archive digest, rejects unsafe
tar entries, re-runs the normal Agent Plugins inspection, publishes the normal
immutable local tree, and only then advances its local cursor. A crash between
local installation and cursor advancement causes a safe repeated verification,
not a duplicate install. Schema v11 stores only the bound registry UUID and
last applied revision.

Synchronization is pull-on-management rather than a hidden background loop.
Install and list use it when the optional registry is configured; connect uses
it only when the requested package is absent locally. The explicit
`renoa-agent plugins sync` command is the administration and deployment check.
No HTTP request is retried inside the client. A visible retry reuses package
digest identity and therefore converges at the service boundary. Existing
connected tools continue to run from local Host state if the package service is
offline.
`AgentSession` owns one Agent/Session binding, canonical workspace, model
catalog, durable model selection, and active-turn coordination. ACP sees these
Host types; it does not construct a kernel `Runtime` or persist Host state.

The Host adds three fixed extension-registry tools to every assembled profile
runtime: `tool_search`, `tool_load`, and `tool_execute`. Search returns at most
200 compact matches without schemas. Load returns only one through three
explicitly requested model-facing schemas. Execute resolves one exact reference
containing the current catalog digest, then reuses the proven MCP credential,
adapter, result, and `NeverReplay` boundary. A missing adapter fails execution
visibly; it does not prevent an Agent from starting or hide searchable catalog
state.

The registry tools open current `host.sqlite3` state for each call. A committed
connection attachment or catalog refresh is therefore visible on the next
search even when the surface process, Agent session, and current turn are
already running. The runtime itself is unchanged: the kernel freezes the same three
registry implementations, while exact references prevent a newer catalog from
silently changing a selected invocation.

The Host adds one fixed `extension_manage` tool. Its v9 binding exposes one
exact, closed schema variant for each of ten typed actions:
search compact publisher metadata in the official MCP Registry; lookup one
exact published Registry name/version; add one MCP definition independently
verified against the provider's official documentation or one content-bound
local Agent Plugins 1.0 directory; inspect a local package; install the exact
inspected digest; list package integrity and durable connection state; connect
one supported package MCP server for the active profile; authorize or
explicitly restart one registered OAuth connection; disconnect one connection
from that profile without deleting its durable package, registration, catalog,
or credential reference; or re-enable that retained complete catalog without a
network request.
Inspection and installation execute no package code. Installation publishes a
full immutable tree at `plugins/<sha256>` before committing its durable record.
Official Registry discovery is a replaceable read-only input to this management
boundary, never executable truth. It verifies publisher namespace control only,
returns explicit coverage, and cannot flow directly into `add`. A local package
add requires the digest returned by inspection so a
crash replay cannot read changed bytes from the same path. Add normalizes every
accepted source into the same package path, installs it, loads supported
package skills, and only then attempts the requested MCP connection. MCP
discovery validates the real endpoint before any tools are published. A
connection or authentication failure returns the retained package digest,
package notices, skill result, and safe exact service error instead of rolling
back unrelated components.
Connect accepts no credential, one named Secret Service bearer or exact header
reference, or Host-owned OAuth, never a raw key. OAuth opens an exact loopback
browser flow, keeps client state and tokens in the desktop Secret Service, and
stores only a deterministic reference, non-secret flow phase, and semantic
terminal receipt in SQLite. It automatically refreshes an expired token under
a cross-process connection lock. A possibly dispatched code exchange or refresh is not retried
after process loss; replay of an already settled management operation reads its
receipt without opening another browser. Explicit `authorize` with `restart:
true` abandons an expired or unknown flow only for a new operation. The Host
discovers and attaches through the same MCP
catalog path used by `LocalHost`; the next `tool_search` sees the connection
without restarting the session or surface. Disconnect is idempotent and the
next search stops exposing its tools while the verified catalog remains
available for recovery or later reattachment. Package skills enter the same
skill registry under a lower-priority plugin scope; workspace overrides global, and
global overrides plugin. A newer revision of the same plugin replaces its
bindings, while a second plugin with the same skill name is visibly rejected.
The next `skill_search` sees a committed package skill without restarting the
session or surface. Model-facing management results use the same 50 KiB
tool-output boundary as local tools and fail instead of silently truncating
package facts. List keeps aggregate state below that boundary by returning at
most 32 compact package, server, notice, connection, and skill facts per page.
Its opaque cursor is bound to the complete inventory revision, so concurrent
changes produce a visible conflict and a fresh first-page requirement rather
than offset drift. Package integrity, durable connection state, profile
attachment, and accepted/rejected plugin skill bindings remain separate facts.

The Host also adds exactly two Agent Skills tools: `skill_search` and
`skill_load`. Search rescans global `~/.agents/skills` and the canonical
workspace's `.agents/skills` on every call, imports each accepted directory into
`skills/<sha256>`, atomically publishes one complete source snapshot, and
returns at most 200 matches containing only `name` and a short `description`. A
source-scan failure keeps the prior snapshot. Invalid individual entries are
stored as rejections without hiding valid siblings. A workspace skill
deterministically overrides a same-named global skill for discovery; no digest,
scope, file list, or other package detail enters the search result.

Load accepts one selected name, resolves the project-over-global binding, and
verifies the Host-owned package before persisting its exact digest. It returns
the complete instructions and a bounded file sample. A concurrent source
change fails instead of switching content during the call. Once activated,
later loads by that name are idempotent for the session-pinned revision even if
the source changes. Skills never grant tools; the experimental `allowed-tools`
field is rejected. Search and load are `SafeToReplay` because their writes
converge on content identity and session uniqueness.

The activation records its originating command. That command receives the full
instructions from the tool result, while a crash retry excludes its own new
activation and therefore reconstructs the same frozen runtime manifest. The
next operation reattaches every active exact revision above the durable
conversation. Prior full `skill_load` results are projected to receipts for the
model, but remain unchanged in kernel history. This survives explicit or
automatic compaction and Host restart. The Host does not impose a policy limit
on the number or total instruction size of skills the user chooses to activate.
Their real cost is visible in the selected model's context usage, and an actual
provider context limit is reported as a provider failure rather than disguised
as a Renoa skill rule.

`LocalRuntimeConfig` is the lower composition input used inside the Host. The
Host selects the session's registered profile before resolving these inputs:

- provider and model;
- reasoning configuration;
- the profile's base prompt, optional bounded workspace `AGENTS.md`, and exact
  active skill instructions; and
- the six workspace tools, three fixed MCP registry tools, one fixed extension
  manager, and two fixed skill registry tools.

`build_local_runtime` resolves that recipe with a `LocalWorkspace`:

```text
LocalRuntimeConfig + registered AgentProfile
  + BridgeModel
  + CompactingContextStrategy
  + LocalWorkspace tools
  + Host MCP, extension manager, and skill registry tools
            |
            v
renoa-agent-loop::build_runtime
            |
            v
renoa-kernel::Runtime + frozen RuntimeManifest
```

The process adapter is both the model implementation and the deterministic
context sizer. The Host derives the same researched compaction limits used by
the existing local product path. Model identity, reasoning, context behavior,
instructions, limits, tool specifications, recovery declarations, and
workspace-bound tool revisions are represented by the resulting manifest.

Model and reasoning selection are not profile or Agent identity. They may change
between operations while the Agent Instance, Session, instructions, tools, and
history remain continuous. A change never mutates an active operation; the
kernel freezes each operation's exact model and reasoning revision.

The Host resolves a fresh runtime for every newly admitted operation. This
re-reads the canonical workspace `AGENTS.md`, so a project-rule edit applies to
the next turn without restarting the surface. It cannot change an operation
that is already admitted because the kernel has frozen that operation's
manifest.

The selected profile identity is durable session state. The recipe itself
remains process-registered configuration until a real profile-management
consumer proves the storage and mutation contract.

## Command path

The first product-owned management command installs the read-only GitHub MCP
connection without putting service policy in the generic Host API:

```sh
renoa-agent mcp github install --account ACCOUNT
```

It registers the exact `github.com` account reference, resolves its token with
`gh` only for discovery, atomically publishes the complete catalog, and attaches
the GitHub connection to Alpha. Repeating the command converges on the same
durable state. The next registry search sees the connection without restarting
Waku or Alpha; no GitHub schema is advertised until explicitly loaded.

The first real Host flow accepts either an ordinary prompt or a typed compact
control:

```text
surface adapter or local caller
  -> LocalHost creates or loads AgentSession
  -> AgentSession accepts one caller-identified command
       -> read current workspace rules
       -> resolve the selected model, context, loop, and tools
       -> LocalSession atomically admits the command
       -> drive that exact operation through the kernel
       -> project a durable assistant or compaction result
```

`Kernel::submit_exclusive` combines the unfinished-operation check and command
insert in one immediate SQLite transaction. `AgentSession` uses this optional
admission primitive because one conversation turn must finish before another begins.
The general kernel `submit` path still permits ordered queues for future
profiles. Exact redelivery remains idempotent; a different command is rejected
without leaving ghost queued work.

`LocalSession` remains the lower shared command boundary used by Agent profiles
and the headless diagnostic runner. Its prompt and explicit-compaction methods share
the same exclusive admission, stable command identity, drive, cancellation,
and durable replay path. `LocalTurnOutcome::Compacted` carries the persisted
post-compaction input estimate without pretending that a control operation
produced an assistant message. `AgentSession` is the complete surface-facing
Host boundary: it also owns runtime selection, persistence, fresh per-turn
composition, and cancellation coordination.

The Host derives context usage from the newest semantic fact in journal order.
A provider-reported assistant usage includes uncached input, cache reads, cache
writes, and generated output because that output becomes part of the next
request. A later compaction result replaces it with the exact projected idle
estimate. A later assistant response without provider usage clears the prior
estimate rather than showing stale surface telemetry.

Surfaces do not call the kernel driver, loop, model, or tools directly. ACP
uses this Host composition and command path. The UI surface consumes that
stable ACP contract rather than creating a second execution path.

For live presentation, the Host may compose an `AgentEventSink` into the model
and tool adapters. That observer is not part of the runtime manifest and does
not replace semantic history. ACP streams these transient events immediately,
then derives final output from kernel events only after durable settlement.

The local Host currently has no reconciliation UI for an effect whose outcome
cannot be proven. For a live MCP call that returns no terminal response, the
Host records an honest model-visible tool result saying that the call may or
may not have succeeded, does not replay it, and lets the same agent turn keep
reasoning. If the process dies before that result is persisted, the kernel's
conservative `OutcomeUnknown` recovery boundary still applies; the kernel
never invents or replays an uncertain external result.

Local Host state has one intentionally visible layout:

```text
<data-root>/
  host.sqlite3                  package metadata, MCP state, credential
                                references, and skill/session bindings
  oauth-locks/<sha256>.lock     process-crash-safe per-connection OAuth lock
  plugins/<sha256>/             immutable Agent Plugin directory
  shared-registry/              owner-only transient package transfers
  skills/<sha256>/              immutable imported Agent Skill directory
  sessions/<session-uuid>/
    session.json                durable identity and workspace/profile binding
    runtime.jsonl               acknowledged provider/model/reasoning selections
    kernel.sqlite3              authoritative execution and recovery truth
    trace.sqlite3               ordered Host/model/tool diagnostics plus exact
                                profile, Agent, and Session identity
```

Usage, cache counts, timings, provider payloads, streamed chunks, and tool
diagnostics belong in `trace.sqlite3`, never `runtime.jsonl` or model context.
Trace rows explain execution but never decide replay or semantic history. A
v1 trace is migrated in place to add the durable Agent and profile identity
already proven by its session manifest.

The Host assembles these files in a hidden directory. After all four are synced
and the kernel lease is closed, it atomically renames that directory to the
session UUID and syncs the parent directory. Initialization failure removes the
staging directory, so a loadable session is never partially published. On Unix,
the published session directory is owner-only because trace and history contain
prompts, source text, and tool data.
The global `host.sqlite3`, `plugins/`, and `skills/` stores are owner-only on Unix.
`runtime.jsonl` recovery truncates an incomplete crash tail before any later
append; future valid records can never be joined onto torn JSON.

## Agent-driven changes

The full intended extension lifecycle and its staged proof plan are recorded in
[`renoa-extensions-north-star.md`](renoa-extensions-north-star.md).

The GUI is a surface, not the sole controller. `LocalHost` methods and each
profile's `extension_manage` tool reach the same `PluginManager`; a future Waku
view will call that Host path rather than own extension state:

```text
human surface --\
                 -> effective session/profile policy -> Host operation
running agent --/                                      -> durable change
```

The current deliberate full-access policy permits search, lookup, inspect,
install, list, connect, and authorize without a second plugin approval prompt.
Service OAuth consent is authentication, not another Renoa permission decision.
A later restricted profile will gate the same management binding through its
one effective permission scope. An agent may exercise that authority but cannot
broaden it.
MCP registry attachments are visible at the next lookup; static runtime changes
wait for a new operation and never mutate an active manifest.

The kernel and the trusted Host enforcement path are outside agent-managed
modification.

## Locked decisions

1. The Host, not a surface or loop, resolves runtime composition.
2. `renoa-local` is the first concrete Host; no competing Host crate is added.
3. Agent identity and runtime assembly remain separate.
4. Profiles, installed capabilities, and resolved runtimes remain distinct.
5. The exact runtime is frozen by the kernel per operation.
6. GUI and agent changes will use the same Host management semantics.
7. V0 exposes all configured local tools and adds no permission model.
8. Provider, workspace, surface, and future permission policy stay outside the
   kernel.
9. The Host registers concrete profiles, persists the exact selected identity
   per session, and fails closed when a required profile is unavailable.
10. Installed packages, MCP catalogs, and immutable skill revisions are one
    Host inventory; access and activation are explicitly profile-scoped.
11. Every trace database identifies its profile, Agent, and Session.
12. The Host owns OAuth coordination, client-registration policy, and secret
    references; the MCP adapter speaks the protocol, while packages, surfaces,
    the loop, and kernel never own credentials.
13. Shared package availability is a Host concern. The package registry carries
    immutable package bytes and ordered revisions only; it never becomes RCP,
    remote execution, credential distribution, profile authorization, or
    surface state.

## Open decisions

- future Host schema migrations beyond the proven v1-through-v11 chain;
- historical resolved-binding retention across explicit catalog/profile
  changes for unfinished-operation recovery;
- explicit skill deactivation, active-revision upgrade, source configuration,
  and immutable-package garbage collection;
- durable profile definition storage, profile inheritance, and Agent Instance
  overrides;
- permission vocabulary, scopes, policy inheritance, and enforcement;
- public package discovery, updates, rollback, removal, and garbage collection;
- the Host management transport and presentation;
- whether capability changes pause and continue a task through one or more
  internal operations; and
- a durable Agent catalog, multiple Sessions per Agent, and process placement
  for multiple concurrent local Agent Instances;
- credential, profile-definition, connection, and attachment distribution
  across Hosts or nodes; and
- surface routing and cross-node continuity, which remain future RCP/product work.

These remain open deliberately. No placeholder contract should make them
appear settled.

## Proven slices

The Host foundation proved that:

1. `renoa-local` resolves the existing model adapter, durable compaction strategy,
   and complete local tool set into a kernel `Runtime`;
2. the local headless runner executes its real coding turn through
   `renoa-kernel`, not the legacy harness;
3. the frozen manifest names the model and all six tool bindings;
4. the existing real workspace edit and Bash cancellation paths remain green;
   and
5. ACP, RCP, package installation, permissions, and UI code remained outside
   that coherent foundation slice.

The next consumer slice is also complete: ACP talks only to `LocalHost` and
`AgentSession`, creates and reloads Alpha identities, admits stable turn IDs,
streams transient model and tool events, durably cancels active effects, and
projects final answers from semantic history. Exact redelivery is proven both
within one process and after restart. Concurrent admission cannot leave ghost
work, pre-cancelled turns are not admitted, unknown effects do not wedge the
session, project instructions refresh per turn, session publication is atomic,
and torn runtime logs remain appendable. Per-turn trace rows preserve ordered
model/tool flow without entering kernel truth. The legacy harness crate is
retired.

The Host is now profile-generic while ACP deliberately remains Alpha-specific.
A deterministic non-Alpha profile reaches the model with its own instructions,
persists its exact profile/Agent/Session trace identity, survives Host restart,
and fails closed when reopened by a process that did not register it. One MCP
catalog can be attached to two profiles without copying it, while attaching it
to one profile alone does not leak access to the other. This prepares the Host
for additional agent recipes without inventing surface or permission policy.

The same Host path now admits explicit compaction as a typed control operation.
Its summary, checkpoint activation, result projection, exact redelivery,
cancellation, and post-restart usage restoration are kernel-backed; no surface
owns or reconstructs that state.

The first extension path is also complete. `LocalHost` registers direct no-auth
or exact `gh`-referenced MCP connections, runs the replaceable Node adapter for
bounded discovery and invocation, atomically publishes catalogs and tool
attachments, and restores them after process restart. Every assembled profile
exposes three fixed registry tools regardless of catalog size. Search and load are bounded
`SafeToReplay` reads; execute carries an exact catalog reference through the
normal loop and kernel as a `NeverReplay` effect. Exact registration retries
converge, identity changes conflict, failed refresh publication preserves the
previous snapshot, stale references fail closed, structured details stay
outside model context, unknown calls are not replayed, and schemas v1 and v2
migrate to v3 without losing catalog state. A live registry object observes a
newly committed attachment, and searching 1,000 tools exposes no schema. No
kernel type or table changed.

The first OAuth connection path is also complete. One `extension_manage`
invocation can register an OAuth package connection, open PKCE browser
authorization, persist the callback before acknowledgement, perform one code
exchange, discover and attach the authenticated catalog, and make it visible
without a restart. Credential state is bound to the exact MCP endpoint and
stored only in Secret Service. Browser cancellation resumes without repeating
registration; concurrent sessions perform one rotating refresh; and a lost
credential exchange becomes durable unknown instead of being replayed. A
completed or definitely failed management operation is receipt-backed, so the
same session/command/tool-call identity causes no second authorization flow.
Host schema v8 adds the connection kind, non-secret recovery phases, and bounded
semantic receipts. Schema v9 adds explicit CIMD, pre-registered-client, and DCR
policy while preserving existing OAuth connections as DCR. Client credentials
remain named Secret Service references; no kernel or RCP type changed.
Schema v10 adds validated generic credential header names and public prefixes;
existing connection kinds migrate without changing identity or catalog state.

The standalone Agent Skills path is now complete. Alpha sees two additional
constant schemas regardless of skill count. Search imports standard global and
workspace `.agents/skills` directories on demand, returns only up to 200
name/description pairs, applies explicit workspace-over-global precedence,
isolates invalid entries, and observes additions without a Host or surface
restart. Load durably pins one immutable revision per name and reattaches its
exact instructions on later operations. A real Alpha session
loads a project skill, hot-loads a newly added skill, compacts, restarts the
Host, and continues with both exact instruction sets. Historical tool results
remain durable while model-facing duplicates become receipts. Schema v4 owns
the shared records; the kernel, ACP, Waku, and RCP receive no skill-specific
storage or protocol path.

The first portable package path is complete. The Host validates Agent Plugins
1.0 manifests locally, isolates invalid or unsupported MCP entries, denies
symlinked fixed components, and publishes exact full trees under a verified
content digest. One fixed `extension_manage` schema drives the same manager as
the public `LocalHost` methods. Schema v6 stores package metadata, public MCP
headers, and only named Secret Service references. Schema v7 preserves plugin
homepage metadata and imports package skills without changing existing source
bindings. An Exa-shaped package is
inspected, installed, connected through the real Node adapter with its public
header and just-in-time bearer, and observed by the existing live registry;
the key never enters Host SQLite. A skill-bearing package is installed and
becomes searchable live; invalid and colliding skills remain visible component
failures. No kernel, loop, ACP, Waku, or RCP type was added.

The first public discovery path is also complete. The Host supervises a
replaceable Node adapter over one bounded versioned process contract.
`extension_manage search` queries the official MCP Registry's stable `v0.1`
API with deterministic multi-word normalization, cursor bounds, no cache, and
no retry; `lookup` accepts only one exact name/version. The Rust boundary
revalidates every normalized result and fixed trust statement. Search exposes
no endpoint, lookup never installs, unsupported transports and packages stay
explicitly blocked, concrete URL credentials are rejected, and safe HTTP
status facts reach Alpha without an untrusted response body. Search uses identity tokens rather than broad
substrings, so an unrelated publisher such as `trycloudflare` is not treated as
Cloudflare. Every management action has an exact schema that rejects fields
from another action. Generic Secret Service headers, idempotent re-enable, and
separate package/connection/skill-source status remain Host behavior, and no
kernel, ACP, Waku, or RCP type changed. List uses bounded revision-bound cursor
pages and rejects a stale cursor if that Host inventory changes.

The current MCP adapter is revision v0.7 on process wire 7. Discovery compiles
each external tool's input schema with the pinned SDK validator and isolates an
invalid definition. Invocation validates the exact arguments against the
frozen schema before dispatch. Header credentials remain standard-input-only,
endpoint-scoped, collision-checked, and redacted; older complete catalogs stay
readable but new discovery publishes only v0.7.

The first shared Host package path is complete. A loopback-only registry owns a
stable identity, content-addressed tar blobs, and one contiguous SQLite revision
log. Two already-running `LocalHost` instances converge through the public Host
management method: one publishes a locally verified package and the other
downloads and independently validates it without restarting. Exact publication
does not create a second revision, executable bits survive transfer, a service
and Host restart resume from the durable cursor without duplicates, a network
failure does not advance that cursor, and a different registry identity is
rejected. No credential, connection, profile attachment, session record,
kernel type, RCP type, or surface contract is copied.
This synchronization path changes the frozen `extension_manage` implementation
from revision 9 to revision 10. An unfinished revision-9 operation fails closed
after upgrade instead of acquiring network synchronization under its old
manifest.
