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

`renoa-local` is the first Host implementation. The graphical surface remains
the separate Renoa integration fork of Waku and connects through ACP. The
repository-owned Telegram surface calls the same Host API directly; it does not
create another loop or history store. Renoa-specific capability management will
use a separate logical Host API whose transport is not selected until a real
management consumer is implemented.

The first concrete coding profile is Renoa Alpha v1, specified in
[`renoa-alpha-v1.md`](renoa-alpha-v1.md). Its stable Host identity is
`renoa.coding.alpha.v1`. Alpha is one built-in profile, not a special Host
execution type. Arcee is the first personal-operator profile, with stable identity
`renoa.personal.arcee.v1`; Telegram is only its first surface. A Host process
registers one or more `AgentProfile` recipes and can create a session from any
exact registered `AgentProfileId`.

Arcee's stable system rules remain part of the registered profile. The Host
seeds owner-editable `SOUL.md` and `USER.md` files under
`profiles/renoa.personal.arcee.v1/` in its data directory and reads both for
every newly admitted turn. `profile_update` replaces one complete document
atomically against the revision shown in the prompt. A stale edit fails without
changing the newer file. Existing files are never overwritten during startup.
The Soul controls Arcee's identity and voice. The User file stores durable facts
and preferences about the user. Neither file changes kernel state or the
surface binding.

Arcee starts with an intentionally empty User file. She learns durable facts
during ordinary work and may update either document through `profile_update`
when the evidence is strong enough. Startup does not interrogate the user or
invent a profile from environment data.

Arcee's profile starts automatic compaction when the exact projected model
input reaches 400,000 tokens. The provider's advertised context window remains
unchanged. Compaction targets a 40,000-token rebuilt request containing the
fixed instructions and tools, a bounded checkpoint, and the latest safe
conversation tail. When no safe transcript cut can meet that target, the
planner keeps the smallest safe tail and still respects the real provider
limit. This policy follows Arcee across surfaces and does not change Alpha's
context policy.

Arcee permits only the OpenCode Go model provider. This is Host profile policy,
not Telegram policy: every surface that opens an Arcee session sees the same
provider boundary. The selected OpenCode Go model and reasoning level remain
session configuration and may change between operations. Host launch
configuration may set the initial reasoning level for new sessions; saved
sessions keep their own acknowledged model and reasoning selection.

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

### Reuse within one Host

A Host is identified by its durable data root and Host UUID, not by a surface
process. Several local surface processes may open that same root under the same
OS identity and compatible Renoa build. They share installed plugin revisions,
MCP connection identities and credential resolution, profile attachments, and
immutable skill files. Each kernel session still has one exclusive execution
owner. An unrelated data root is a separate Host even on the same machine.

`extension_manage list` inventories the Host's packages and connections and
reports whether each connection is enabled for the calling profile. `enable`
attaches an existing connection without reinstallation, rediscovery, or another
credential ceremony. Every surface using that profile sees the same attachment.
A different profile selects the connection explicitly; availability in the Host
library does not automatically expose every tool to every recipe.

To reuse a package's skills, call `add` with
`source: {"kind": "installed", "package_digest": "<digest from list>"}`.
The Host verifies that exact installed revision and attaches its supported
skills to the profile without needing the original source directory or a
network registry. This does not connect the package's MCP servers. Existing
connection identities are reused through `enable`; a new connection remains an
explicit separate operation. Missing or corrupt revisions fail rather than
being downloaded or substituted. This extends the frozen extension manager
binding from revision 16 to 17; unfinished older operations retain the existing
fail-closed runtime compatibility behavior.

Global skills are discoverable by each profile through the configured global
source; workspace skills keep their workspace scope. Loading a skill pins its
exact revision to that session. Sharing the library does not share conversation
history or silently activate another session's instructions. Cross-machine Host
access and credential sharing between distinct Hosts remain separate work.

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

Built-in profiles are immutable process configuration: a stable validated identity,
base instructions, an optional exact model-provider restriction, and whether
the canonical workspace-root `AGENTS.md` is included. A session manifest persists the selected profile identity beside its
Agent, Session, and workspace identity. Loading fails closed when that exact
profile cannot be resolved from process configuration or the Host catalog.
Specialist recipes are stored in `host.sqlite3` and resolved by already-running
Host processes. Both kinds remain immutable in this slice.

The Host catalog stores one stable Host UUID and durable Agent records: an
Agent ID, registered profile identity, display name, and optional creating Agent.
Creation uses a caller-supplied identity and is idempotent only for identical
fields; conflicting creation data or an absent creator fails. Creator links are
immutable and reference existing agents, so they cannot form cycles.
`ensure_agent_session` binds an exact caller-supplied Session ID to an existing
Agent. Several sessions may share that Agent while retaining independent
workspaces, model selections, histories, and exclusive execution ownership.
Deleting a session does not delete its Agent record.

The older create/ensure-session APIs retain their one-new-Agent-per-session
behavior. Published session manifests remain authoritative for their Agent,
profile, and workspace binding; loading a legacy session or listing the roster
imports its existing identity into the catalog without opening a model or trace
store. Catalog publication may follow session publication: a crash in between
is reconciled from the existing manifest, without creating another Agent.
The catalog records identity and composition selection, not a second execution
journal. Kernel operation facts remain the execution authority.
The [Slack adapter](../crates/renoa-slack/README.md) consumes the durable Agent
management path: its Arcee Agent survives restarts, while DMs and channel
threads bind independent conversations through `ensure_agent_session`. Its
transport admission and reply receipts remain surface-owned. Specialist recipe
creation now uses the same Host management path; routines use the same Host identities and execute independently of surfaces.

Telegram, Slack, WhatsApp, ACP, a GitHub webhook, and a GUI are surfaces or ingress
adapters; they do not become profiles merely because they deliver messages. A
GitHub-review recipe or a daily-assistant recipe is a profile and may be used
from any compatible surface.

## Full-access first slice

Permission semantics are deliberately open. V0 does not introduce roles,
levels, grants, approval records, or a permission trait.

Profiles run with full access through the tools selected for them. Built-in
profiles advertise all local workspace tools; specialist recipes select a subset.
This selection does not add an OS sandbox or a permission system. External catalogs are
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

Arcee also receives `bot_manage`; specialists receive it only when selected
in their recipe.

Existing tool invariants remain in force. File tools stay within the configured
workspace. Bash starts in that workspace but is unrestricted and is not a
sandbox for untrusted work. "Full access" means no new Host approval system; it
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

Revision-checked file edits and profile updates coordinate cooperating Renoa
writers, including separate processes, with a persistent `.renoa-lock-<digest>`
sidecar in the target's canonical parent directory. The OS lock covers revision
validation, replacement, and parent-directory sync; its empty sidecar must not
be deleted while writers may be active. Process exit releases ownership.
Distinct replacements based on one revision cannot both succeed. Identical
profile retries still succeed. Unconditional `write_file` also takes the lock
but intentionally overwrites the latest contents without a revision check.
This coordination does not prevent arbitrary external programs from changing
files or removing the lock sidecars.

This repository ignores `.renoa-lock-*`. Other Git workspaces can put the same
pattern in their `.gitignore` or local Git exclude file; Renoa does not modify
their Git policy automatically. Ignoring sidecars keeps them out of ordinary
staging without relocating or unlinking the inode that coordinates writers.

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

`LocalHost::inspect_session` opens existing durable history without provider
catalog discovery, saved-model validation, or runtime construction. Its
`AgentSessionHistory` handle retains exclusive kernel ownership and exposes a
separate diagnostic-store error without hiding intact history. Inspection checks
the registered profile, exact session/Agent identity, canonical workspace binding,
and authoritative data integrity. It cannot execute or recover a turn; callers
drop the handle and use normal executable loading after repairing dependencies.
ACP uses this path when normal session loading is unavailable.

`host.sqlite3` schema v19 keeps Host and Agent identity records, installed package metadata, supported package MCP entries,
direct integration and connection identities, non-secret credential references,
durable non-secret OAuth phases and terminal receipts, complete MCP catalog
snapshots, per-profile attached connection identities, immutable skill revisions,
source/profile bindings, rejected skill entries, and session activation pins.
Registration, discovery, and profile attachment remain separate states.
Catalog replacement and attachment are transactional, and multi-query reads use
one SQLite snapshot so a registry call cannot observe half of a refresh.
One shared authorization resolver is composed into management, discovery, and
runtime tool execution. Desktop composition selects loopback callbacks and
Secret Service; headless composition selects the self-hosted callback relay and
the Host's private file-backed secret facility. The same headless composition
also enables end-to-end encrypted browser intake for a missing API token or
pre-registered OAuth client. The coordinator retains ciphertext only; the
requesting Host keeps the decryption key and stores the resulting credential.

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

The Host adds one fixed `extension_manage` tool. Its v18 model-facing schema is
flat and uses only the broadly supported JSON Schema subset needed by
OpenAI-compatible providers. The Host still decodes one exact, closed variant
for each of ten typed actions and rejects missing or cross-action fields:
search compact publisher metadata in the official MCP Registry; lookup one
exact published Registry name/version; add one MCP definition independently
verified against the provider's official documentation or one content-bound
local Agent Plugins 1.0 directory; inspect a local package; install the exact
inspected digest; list package integrity and durable connection state; connect
one supported package MCP server for the active profile, optionally carrying
an exact scope from a prior `oauth_insufficient_scope` result; authorize,
scope-upgrade, or explicitly restart one registered OAuth connection;
disconnect one connection from that profile without deleting its durable
package, registration, catalog, or credential reference; or re-enable that
retained complete catalog without a network request.
The model-facing descriptions state the remote-MCP setup sequence at the
fields where a model makes each choice. In particular, `add` may connect in the
same call and browser authentication is exactly `credential.kind=oauth`. The
Host, not the model, discovers the issuer, chooses a metadata-supported client
mode, derives the credential reference, and emits any secure credential page
before the provider sign-in page. This guidance changes only the frozen Agent
binding. It does not add a Host, kernel, MCP, or RCP field.
Inspection and installation execute no package code. Installation publishes a
full immutable tree at `plugins/<sha256>` before committing its durable record.
Official Registry discovery is a replaceable read-only input to this management
boundary, never executable truth. It verifies publisher namespace control only,
returns explicit coverage, and cannot flow directly into `add`. A local package
add requires the digest returned by inspection so a
crash replay cannot read changed bytes from the same path. Add normalizes every
accepted source into the same package path, installs it, loads supported
package skills, and only then attempts the requested MCP connection. Package
installation and connection activation remain separate reported facts. MCP
discovery validates the real endpoint before any tools are published. A
connection or authentication failure returns the retained package digest,
package notices, skill result, and safe exact service error instead of rolling
back unrelated components.
Connect accepts no credential, one named Host bearer or exact header
reference, or Host-owned OAuth, never a raw key. On a configured headless Host,
a missing named API token or pre-registered OAuth client emits a permanent
surface link to the encrypted Renoa credential-intake page and waits. The
browser encrypts locally; the coordinator cannot read the submitted value. The
Host stores and validates it before authenticated discovery. OAuth opens an exact loopback
browser flow on desktop. A configured headless Host instead emits the provider
link to its surface and receives the callback through the short-lived relay at
the Renoa HTTPS origin. Client state and tokens stay in the desktop Secret
Service or the headless Host's owner-only secret directory. SQLite stores only
a deterministic reference, non-secret callback identity and phase, and
semantic terminal receipt. It automatically refreshes an expired token under a
cross-process connection lock. A possibly dispatched code exchange or refresh is not retried
after process loss; replay of an already settled management operation reads its
receipt without opening another browser. Explicit `restart: true` on `authorize`
(registered connections) or `connect` (including unpublished attempts) abandons
an expired or unknown flow only for a new operation. The latter reuses the saved
package and client credentials. Replay of the same restart retains its callback
and state rather than opening another authorization flow. The Host
selects initial OAuth scopes from the first challenge, then protected-resource
metadata, as required by MCP. A later HTTP 403
`insufficient_scope` result is a definite, model-visible failure carrying the
server's exact validated scope. `extension_manage` unions that scope with the
stored grant and opens fresh consent when it widens permission. It never
silently retries the denied MCP call; the Agent must authorize and then issue
one explicit retry. Registration modes are not model input or fallbacks to
guess: strict endpoint metadata must name one issuer; the Host then chooses an
existing issuer-bound client, hosted CIMD when advertised, DCR when advertised,
or a developer-console client form already bound to that issuer. The
headless setup form's frozen wire spelling is `oauth_client`; coordinators also
accept the short-lived buggy `o_auth_client` spelling only for rolling upgrade
compatibility.
The Host discovers and attaches through the same MCP catalog path used by
`LocalHost`; the next `tool_search` sees the connection
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
Revision 12 freezes encrypted credential intake and transactional connection
publication. An unfinished revision-11 management call fails closed after
upgrade instead of resuming across the changed effect boundary.

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
Profiles may restrict which configured providers are eligible without fixing a
particular model. Discovery, loading, and later model changes all enforce the
same restriction.

The Host resolves a fresh runtime for every newly admitted operation. This
re-reads the canonical workspace `AGENTS.md`, so a project-rule edit applies to
the next turn without restarting the surface. It cannot change an operation
that is already admitted because the kernel has frozen that operation's
manifest.

The selected profile identity is durable session state. The recipe itself
remains process-registered configuration until a real profile-management
consumer proves the storage and mutation contract.

## Command path

Local management commands use the same typed Host operations as future
surface and model-facing callers, and emit JSON:

```sh
renoa-agent agents list
renoa-agent agents show AGENT_UUID
renoa-agent agents ensure AGENT_UUID NAME [CREATOR_UUID]
renoa-agent agents session AGENT_UUID SESSION_UUID /absolute/workspace
```

These commands use the existing `RENOA_DATA_DIR` and provider launch settings.
`ensure` selects the CLI's registered Alpha profile; arbitrary recipe editing is
not implemented. Listing, lookup, and creation need no executable model bridge.
Session creation resolves the real registered profile and model normally, and
the returned Session ID can be reopened through the existing ACP load path.
Host and Agent identity, creation retries, parent relationships, and isolated
multi-session execution are tested across restart. The Host UUID identifies a
durable data root; this slice does not replicate catalogs across machines.

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
  -> LocalHost creates, ensures, or loads AgentSession
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

Profiles may opt into Host turn timing. A direct caller observes the Host clock
once; a queue-backed surface supplies the receive time it already persisted.
That observation is serialized in the exact command before admission. A retry
with the same command identity reuses the admitted value, even if the process
restarts or the caller now observes a different time. The next timed prompt
computes elapsed time from the newest admitted timed user message. If the clock
moves backward, elapsed time is omitted instead of fabricating a duration.

The Host formats the observation in the operating system's configured time
zone. `TZ` may override it for one service, with UTC as a safe fallback. The
loop appends it to the matching user message as
`<turn_context>` for the model. It is not inserted into the changing system
prompt and it is not copied into surface history. Every later request rebuilds
each prior turn with the same durable suffix, which preserves the exact prompt
prefix used by provider caches. Alpha remains content-only; Arcee opts into
this behavior.

`LocalHost::ensure_session` accepts a caller-chosen UUID for durable surface
admission. Repeating it resolves the already-published session with its stored
profile, model selection, Agent identity, and workspace instead of validating a
new-session default or creating an orphan replacement. Publication remains an
atomic hidden-directory rename under a process-shared creation lock.

The Host derives context usage from the newest semantic fact in journal order.
A provider-reported assistant usage includes uncached input, cache reads, cache
writes, and generated output because that output becomes part of the next
request. A later compaction result replaces it with the exact projected idle
estimate. A later assistant response without provider usage clears the prior
estimate rather than showing stale surface telemetry.

Surfaces do not call the kernel driver, loop, model, or tools directly. ACP and
the Telegram adapter both use this Host composition and command path. The UI
surface consumes the stable ACP contract rather than creating a second
execution path.

For live presentation, the Host may compose an `AgentEventSink` into the model
and tool adapters. That observer is not part of the runtime manifest and does
not replace semantic history. ACP and Telegram project these transient events
for presentation, then derive final output from the Host only after durable
settlement.

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
  oauth-secrets/<sha256>.json   headless-only owner-protected credentials
  credential-relay-state/      owner-only pending encrypted-intake identities
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

Usage, cache counts, execution timings, provider payloads, streamed chunks, and
tool diagnostics belong in `trace.sqlite3`, never `runtime.jsonl` or model
context. The admitted user-turn observation described above is the narrow
exception: it is semantic model context, not diagnostic trace timing.
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

### Persistent specialist agents

Arcee now creates persistent specialists through the `bot_manage` tool, backed
by typed Host operations. The Host atomically stores a bot's identity, creating
Agent, immutable recipe (initial name, instructions, selected local tools and existing
MCP connections), Agent record, and connection attachments. Every attachment
must have a complete discovered catalog. Creation derives a stable identity
from the kernel tool-call identity; identical replay returns the existing bot,
while conflicting data fails. Cancellation is checked after acquiring the
write lock and before committing, so abandoned creation does not publish a bot.

Already-running Host processes resolve these persisted profiles without a
restart. The selected instructions replace the operator's instructions, and
selected workspace tools are the actual advertised tools. Registry and skill
execution retain the existing Host capability path; selecting `extension_manage`
allows reuse of installed packages and existing connections without repeating
OAuth. Selecting `bot_manage` allows a specialist to create descendants. These
are capability choices, not a permission or OS isolation guarantee.

`list_bots` and the model-facing list operation return compact pages of 20
identities, names, and creator relationships with a continuation cursor. Exact
recipe lookup is separate. Profile inventory includes persisted specialists.
`bot_manage rename` edits the Host display name with an expected-current-name
check and a durable operation receipt. Replay returns the original rename result;
replaying creation retains its immutable recipe without reverting the current
name. Agent identity, sessions, workspace, tools, and connections remain intact.
Names should be short job labels, such as X Desk, News, or Research. Slack
projects display-name changes onto the existing channel by its retained ID.
Slack projects the Host specialist inventory into dedicated private channels and
invites the operator. Its own catalog records provisioning state and conversation
bindings; creation recovery and external channel IDs remain surface concerns.
Plain messages in a dedicated channel route to that specialist. The adapter
also persists a concise interface-capability snapshot with each admitted prompt,
so the model can distinguish automatic surface behavior from connected MCP
capabilities. This remains surface-owned input, not Slack policy in the Host or
kernel. Shared profile memory must not determine the active message surface. Slack also
supports optional `!agent <id>` routing in DMs and ordinary threads: admission persists the
target Agent alongside a fresh Session before acknowledging the command. Earlier
queued requests retain their targets. `!new` keeps the selected Agent; `!agent arcee` returns to the operator in a fresh conversation. A separate channel
thread can keep another conversation open. Specialist working directories live
under the Host's `bot-workspaces/<agent-id>` directory.

The following behavior describes the remaining product direction. Recipe edits and structured generated-artifact management remain open. Host-owned
routines and Slack result delivery are implemented below.

For example, the user asks Arcee to create a news-digest agent with selected
sources, research tools, and a document-generation capability. Arcee uses Host
management operations to create the specialist's recipe and durable Agent
Instance, select available capabilities and account connections, and request a
Slack conversation binding. If a needed capability is missing, existing
extension management supplies the installation/authentication path; mentioning
a tool in instructions never makes that tool available.

The specialist is independently addressable and retains its own conversations,
working preferences, and output references. It can answer a direct message or
run a standing digest task on a schedule. It remains present when Arcee's
creating turn finishes, when either agent is idle, and across Host restarts.
A temporary delegated run may reuse agent execution primitives, but completing
that run and deleting a persistent specialist are different lifecycle actions.

The Host must retain distinct relationships:

- the recipe describes the specialist's instructions and selected components;
- the Agent Instance identifies the persistent specialist;
- the creating-agent relationship supports the operator/specialist hierarchy
  without making the creator's current session own the specialist's lifetime;
- a surface binding maps an external conversation to the intended agent and
  session; the Slack channel is not the Agent identity;
- a routine supplies a standing request, schedule, timezone, and delivery
  destination; each occurrence submits ordinary identifiable work; and
- an output reference identifies a durable artifact independently of its
  Slack attachment or notification.

The personal Renoa installation owns these records. A Slack application may
route several specialists through separate channels or threads; creating a
specialist does not require creating another Slack application or bot token.
The control panel may group agents by surface and nest specialists beneath
their creator, while the underlying identities remain independent of that
presentation and can acquire another surface binding later.

Human controls and model-facing management tools must invoke the same typed
Host operations. Arcee can create and configure the specialist; the specialist
can update its own routine in response to the user's instruction. Changing
"daily at 16:00" to "daily at 14:00" updates the existing routine with an
explicit timezone and reports the next occurrence. It must not silently add a
second routine or mutate an already executing occurrence. A scheduled request
and an interactive request use the same session-admission and ordering rules.

Creation spans local durable records and external surface actions. Retrying an
interrupted creation must resolve the same specialist and routine, reconcile
surface provisioning, and expose partial failure without claiming a usable
channel exists before its binding is confirmed. A routine update needs a
durable identity and revision check so concurrent edits cannot overwrite one
another unnoticed. These guarantees must be tested through actual Host
management callers rather than implemented only in an operator's prompt.

The first complete Slack milestone must prove: Arcee creates one news
specialist; the user talks to it directly; a manual and a scheduled digest use
its configured capabilities; a generated document remains retrievable; the
user changes the schedule through conversation; and restart preserves the
agent, its relationships, and the updated routine without duplicate creation
or admission. The parent/child roster must be inspectable through management
operations before the control panel visualization is built.

This slice retains the deliberate full-access starting policy through the
configured tools and OS/workspace environment. Approval dialogs, automatic
review, general workflow graphs, and execution migration are not prerequisites.
Exact storage schemas and wire fields remain implementation decisions and are
introduced only with a consuming execution path or invariant test.

### Host-owned routines

`routine_manage` and `LocalHost::manage_routine` share typed creation, revision-checked
replacement, and manual-run operations. Arcee may manage any persistent specialist;
each specialist receives routine management for itself, including specialists whose
recipes predate this tool. This is Host management policy; selecting workspace tools
and account connections remains independent. List returns bounded pages with exact
routine IDs, standing tasks, timing, enabled state, revision, and next due time. Model-facing lists omit full standing tasks; `get` reads one
complete routine for inspection or editing.
Pausing sets enabled=false; it leaves an already admitted occurrence intact.
`delete` requires the current revision and records a durable deletion marker in
Host schema 19 while disabling the routine and incrementing its revision. Deleted
routines disappear from listing/get and reject update/manual-run operations; they
cannot be re-armed. Already admitted runs finish, and their results remain readable.
The original routine row and operation receipts remain for run references and exact
replay; replaying creation does not resurrect a deleted routine. Deletion and its
receipt commit atomically, and cancelled/stale/unauthorized requests change nothing.

Host schema 16 introduced routines, management receipts, and occurrence records. A tool
operation derives its stable identity from the session, command, and tool call.
Replaying a management operation returns its original result even after a later edit;
conflicting input and stale revisions fail. Creation targets an existing Host
specialist, not a Slack channel. No surface identifiers or credentials appear in
routine records. Results belong to the agent's durable Host inbox.

The `renoa-host <config.json>` process owns one scheduler lease per Host directory.
It admits and runs one occurrence at a time, without requiring Slack or Telegram.
Admission persists the occurrence ID, exact task, target Agent, execution Session,
scheduled time, and actual admission time before execution. Advancing the schedule
commits in the same transaction. Daily schedules require an IANA timezone; repeated
fall-back times run once and nonexistent spring times shift forward across the gap.
Elapsed-hour intervals retain their original phase. Downtime coalesces missed times
into one catch-up occurrence. A manual run retains the normal recurring schedule and
cannot overlap another admitted occurrence of that routine.

One-time schedules use `{"kind":"once","at":"2026-09-08T14:00:00+05:30"}`.
The timestamp must include an explicit UTC offset or Z, and must be in the future
when creating or re-arming an enabled task. Relative requests are resolved by the
agent against the current date/time and the user's timezone. Disarming and
incrementing the revision commit together with the only timed occurrence's
admission; the retained due time is historical while enabled=false. An overdue
armed task catches up once. A crash resumes its admitted run even though it is
already disarmed. Results and routine records remain available afterward.
`run_now` also disarms a one-time task, avoiding a second run at its original time;
a fresh explicit `run_now` may run a disabled task again. Pausing and editing an
unchanged overdue task are allowed. Re-arming requires a future timestamp and the
current revision; admitted work is unaffected by subsequent edits.

`routine_results` exposes completed-run summaries and exact run lookup through
Host APIs. Specialists can inspect their own runs; Arcee can inspect any specialist.
Listing is bounded to 20 results, newest first, with sequence pagination; exact
lookup returns the retained task, output, and execution-session identity. This path
does not execute the routine and remains available from any surface.

Each routine has a stable execution session, separate from interactive chats, with
the same specialist recipe, workspace, and selected Host connections. Its standing
request must contain the recurring job's requirements; interactive chat history is
not implicitly copied into it. Admission time enters the existing durable user-turn
time context, preserving the system/tool cache prefix. The kernel remains the
execution authority: after a crash, the runner reuses the admitted command and
recovers the kernel outcome. The Host stores that outcome before surface delivery.
Infrastructure errors retain pending work for service restart. Graceful shutdown
drains the current turn; an interrupted process recovers through the kernel.
Unattended credential/OAuth prompts stop the scheduled turn and report that account
setup must be completed interactively, preventing a hidden consent wait from blocking
the scheduler.

At Slack chat admission, schema 8 appends up to eight newly relevant delivered chunks (4,000
characters each) and their run IDs to the durable user-turn context. Selection
requires the same channel and Agent, and only confirmed sent chunks qualify.
The snapshot and its result references commit with the request before acknowledgment;
replay cannot add later outputs or change the admitted input. Completed uncancelled
turns suppress subsequent duplicate insertion in that session; fresh sessions can
recover recent results. Cancelled or still-queued turns do not consume this context.
Earlier system/history prefixes stay unchanged. Full/older outputs remain accessible
through `routine_results`, including after compaction or when delivery is uncertain.

Slack projects completed Host results into its own durable outbox. Projection and
cursor advancement commit together, even if a channel is not ready. Delivery resolves
each agent's ready channel binding; one unbound bot does not block other bots.
The adapter marks posting intent before calling Slack; rate limits retry and uncertain
posts remain unknown rather than being blindly duplicated. Slack downtime delays
notification while the Host continues execution. Other surfaces can consume the same
Host result API with their own delivery cursors. This slice delivers text and durable
workspace file references; binary artifact upload and general workflow graphs remain
separate work. Bot files remain retrievable through that bot's configured file tools.

The daemon launch JSON contains `data_directory`, `model_bridge`, `providers`,
`provider`, `model`, `model_auth_store`, and optional `reasoning`, `mcp_adapter`,
`mcp_registry_adapter`, `shared_plugin_registry`, and `oauth_relay` (origin and private
device credential path). These are Host settings; there are no Slack tokens or
channel IDs. `deploy/renoa-host.service` runs this process independently of surfaces.
The supplied systemd unit loads `/etc/renoa/host.json` as `host-config` and the
shared relay device credential into its own credential directory. For this unit,
set the relay credential path to `/run/credentials/renoa-host.service/oauth-relay-device`;
it must not point into a surface service's credential mount.
Host schema 17 adds durable display-name edit receipts.
Schema 18 admits the one-time schedule variant; older readers cannot decode it.
Schema 19 adds routine deletion markers consumed by listing, lookup, and admission.
All processes sharing the Host must support schema 19 before restarting them after
the migration. The integration tests exercise model-driven creation, specialist
rescheduling, artifact generation, and recovery after losing the Host outcome receipt
without repeating the kernel's completed file operation.

## GitHub reviewer composition

The Host implements repository policy, durable review-request admission, a
disposable inspection executor, and durable GitHub publication. The GitHub
service receives signed webhooks behind HTTPS ingress and supervises separate
review workers. Browser management remains a design target; the browser
currently projects RCP tasks. This
composition adds no GitHub-specific types to the kernel and does not settle the open
RCP wire boundaries.

### Admission boundary

`LocalHost::manage_github_review` is the trusted local management boundary.
`SetRepository` binds a stable GitHub repository ID and installation ID to an
existing Host specialist, an informational `owner/repository` name, enabled
state, selected triggers, and draft handling. Creation uses no expected
revision; updates require the exact current revision. A stable operation UUID
and its exact result are committed together. Reusing an operation UUID with
different input conflicts; retrying an old edit returns its original result
without restoring obsolete policy. Agent or browser callers still need an
authenticated authorization adapter before this local API can be exposed.

`Request` admits an explicit manual request, including while automatic reviews
are paused. `Repositories` and `Requests` return at most 20 records, ordered by
repository ID and admission sequence respectively. Continue with the last
record's ID or sequence. Each request retains its original policy snapshot and
reported base/head commits. They are admission evidence, not a claim that an
executor reviewed those commits or that the latest-arriving event is newest.

`LocalHost::admit_github_review_webhook` validates HMAC-SHA256 over the original
body, limits the payload to 1 MiB, checks installation identity against policy,
and atomically stores a receipt with any new request. Filtered events retain
their original ignored outcome on replay. Delivery UUIDs deduplicate transport
retries; repository/policy revision/PR/base/head identity deduplicates separate
automatic events requesting the same work. Manual requests have their own
operation identity and can intentionally request another review. Request
admission is bounded to 1,024 pending requests; capacity failure acknowledges
no new work, while already-admitted requests remain replayable. Terminal review
outcomes release inbox capacity without deleting requests or their receipts.

Host schema 20 added `host_review_repositories`, `host_review_operations`,
`host_review_requests`, and `host_review_deliveries`. Schema 21 adds
`host_review_runs`; schema 22 adds `host_review_jobs` (absolute lifetime and
publication backoff) and `host_review_publications` (intent and remote outcome).
Schema 23 adds worker-entry evidence, execution retry timing and the last job failure.
Existing Host, specialist, session, capability, routine and
admission records are preserved. All processes sharing the database must support
schema 23 before opening it with these binaries.

The GitHub service verifies at startup that its worker configuration resolves to
the same canonical Host database as the supervisor. Separate model configuration
files and filesystem aliases are allowed; a different Host database is rejected
before loading App credentials or accepting webhook traffic.

The local CLI exposes the same operations without a browser:

```text
renoa-host /absolute/host.json ensure-bot /absolute/bot.json
renoa-host /absolute/host.json github-review /absolute/request.json
renoa-host /absolute/host.json github-webhook /absolute/envelope.json
```

A bot file contains the existing `BotRecord` shape: `id`, `created_by`, and
`recipe` (`name`, `instructions`, `tools`, `connections`). `ensure-bot` calls
the same durable specialist creation operation as the agent tool. The creator
must already exist in this Host. Repeating the same record is idempotent;
reusing an ID with a changed recipe conflicts. It starts no model or surface.

A request file is a serialized `GitHubReviewCommand`, for example
`{"action":"requests","after":0}` or
`{"action":"repositories","after":null}`. The webhook envelope contains
`delivery_id`, `event`, `signature`, `body_file`, and `secret_file`. File paths
must be absolute. The body file contains the exact signed bytes; the private
secret file contains the exact secret bytes (no automatic whitespace trimming).
The CLI never prints the secret or original payload. This is a local admission
and recovery path. The separately launched `github-service` supplies HTTP admission.

### Disposable review execution

`LocalHost::execute_github_review` executes an admitted request under the Host's
exclusive `.reviews.lock` process lease, independent of the routine scheduler:

```text
renoa-host /absolute/host.json github-execute /absolute/execution.json
```

The execution file contains `request_id`, `app_jwt_file`, and
`workspace: {"bubblewrap":"/usr/bin/bwrap","worker":"/opt/renoa/review-tools/<commit>/renoa-workspace-tool"}`.
The worker is the release build of the existing workspace tool binary, installed
at an immutable versioned path. Bubblewrap 0.12.0 or later and unprivileged user
namespaces are required. The JWT is a private absolute file containing a currently
valid GitHub App JWT. The GitHub service signs it immediately before dispatch;
the RSA key stays in that service's systemd credential directory.
The executor verifies the App installation and repository
identity, then mints a token restricted to the repository and read-only contents,
pull requests and checks. Credentials stay outside model context and results.

Before inference, the Host reconciles the PR and freezes base/head and merge-base
commits, repository policy, specialist instructions, model specification and
reasoning. Applicable base AGENTS.md files supply conventions. PR instructions
are review material. Initial context includes changed-file patches, head paths
and observed CI status.

The Host materializes base/, head/ and merge_base/ checkouts under
`review-workspaces/<request-id>`. Git credentials go only to the trusted fetch
process and are not stored in Git config; hooks are disabled. Each inspection
call launches a fresh Bubblewrap sandbox with the checkout mounted read-only,
the workspace tool, ripgrep and its system libraries. It has isolated namespaces,
no network, no capabilities, an empty environment and no Host data or credentials.
Nested user namespaces are disabled. The tool process exits after its response;
there is no persistent sandbox process during model reasoning. This shares the
operating-system kernel and is not a microVM;
the initial deployment serves the owner's personal review workflow.

The Host assembles the named specialist recipe with `review_instructions.txt`,
the shared Rust model/tool loop and the existing replaceable compaction strategy.
`renoa-workspace-tool` executes the same read_file, grep and find implementations
as local agents; only their transport changes. No generic assistant/coding
profile is inherited. Bash, dependency installation, test execution, automatic
fixes and unrelated Host connections are unavailable in this version.

Investigation and validation run until completion, cancellation, failure or the
explicit 60-minute review deadline. There is no model-response count limit.
Each provider call, including silent reasoning, can take up to 30 minutes;
the Node adapter no longer imposes its shorter SDK default. Tool batches allow 50 calls.
The output allowance is 32,768 tokens, bounded by the provider's supported output.
Working input targets 272,000 tokens, automatic compaction starts at 258,400, and
the post-compaction target is 155,040. Smaller model windows lower those settings
after reserving output and safety headroom. Compaction can repeat within either
stage while preserving the transcript and active task. The existing two attempts
per malformed summary are validation retries, not a limit on compaction cycles.
Provider retry semantics remain unchanged. Prefixes and provider session identity
remain stable. Recorded token usage includes summary responses; incomplete
accounting remains unknown.

Each stage is a durable command in `review-sessions/<request-id>/kernel.sqlite`.
Settled stages replay before model resolution. A final Host commit failure does
not repeat completed inference. Unfinished read/model effects retain the kernel's
safe-to-replay semantics; a crash may repeat unacknowledged inference and cost.
The Bubblewrap version and worker binary hash participate in runtime tool bindings,
so an incompatible tool deployment cannot silently resume an active command.

New findings require P0–P3 priorities and are sorted by priority. Legacy reports
without a priority remain readable without assigning an invented one. Validation
checks added-line anchors, required fields, duplicate anchors and exact evidence
against the immutable head checkout. These checks do not prove semantic correctness.
A final PR/policy check retains findings as superseded when the target changed.

The Host saves the result before removing the checkout. A recovered execution
recreates its inspection environment from the same commits. Cleanup failure is
reported; retrying a completed run reconciles leftover workspace resources
without rerunning inference. Durable transcripts and findings survive cleanup.

### GitHub service, recovery and publication

`renoa-host <host.json> github-service <service.json>` binds a loopback HTTP
listener at `/v1/github/webhook`. The ingress exposes only that path. Signature
verification and the Host transaction finish before HTTP 202; no model runs in
the request handler. The service scans GitHub's retained delivery history every
five minutes and on first startup, following authenticated same-endpoint pages.
Failed deliveries without a durable Host receipt are requested again from GitHub;
the replay uses the same delivery GUID. The next scan time is committed before
API calls and survives restart. This relies on GitHub's delivery retention window;
an outage beyond that window needs an explicit review request. Scanning the whole
retained window avoids assumptions about webhook ordering or cursor monotonicity.

The dispatcher runs one review at a time without blocking Host routines or chat
surfaces. Queued automatic requests for older heads are skipped using the current
GitHub PR state; late webhook arrival cannot displace a newer commit. A manual
request still reconciles the live PR before freezing input.

Before launch, the Host records the absolute deadline. A separate systemd user
service owns each review's process group: `RuntimeMaxSec` uses the remaining
deadline, `KillMode=control-group` covers model bridges and tool descendants, and
`TimeoutStopSec=30s` allows cooperative cancellation before forced termination.
`ExecStopPost` calls the Host reaper after exit, timeout or crash. It removes only
that request's checkout and temporary JWT/launcher files under the review lease,
and records an incomplete outcome when the worker left none. Dispatcher startup
also reconciles jobs without a live unit, covering a reboot or failed launch.
The user manager has lingering enabled, so a receiver crash cannot abandon its
worker's lifetime enforcement. No completed transcript is removed.

Publication uses a fresh write-scoped installation token outside the model loop.
The Host persists the exact payload before POST and rechecks the PR's current
head and repository policy. Reviews are bound to the frozen commit, use P0–P3
inline findings, and disclose incomplete execution or coverage. An uncertain
POST is reconciled by bot identity, commit and exact body including a stable Host
request marker. It never causes a blind second POST. An unresolved result is
`needs_attention`; operator investigation is required before another request.
Publication errors retain the completed model result and a durable retry time,
with at least five minutes of backoff and longer GitHub retry/reset hints honored.

`{"action":"publication","request_id":"<uuid>"}` through `github-review`
retrieves sending, published (review ID and URL), suppressed or attention-required
state. Individual comment IDs and conversational PR replies remain follow-up
work; publication currently creates a single GitHub review with inline comments.

`{"action":"run","request_id":"<uuid>"}` through `github-review` retrieves the
prepared snapshot or immutable outcome (reviewed, superseded, skipped, incomplete)
without GitHub credentials or inference. Recoverable preparation/API failures
hand the attempt back to dispatch with persisted backoff and the specific cause,
clearing worker-entry evidence for the next attempt without extending the original
deadline. Cleanup preserves that handoff. Rate limits, transient HTTP failures,
changing PR context and cooperative interruption may retry; invalid credentials,
configuration and other permanent failures retain a specific incomplete outcome.
Explicit terminal model outcomes are not retried; rerunning a terminal review
requires a new request identity. Abrupt worker death without a durable handoff
still produces an incomplete outcome after cleanup.

Initial API preparation still accepts up to 500 changed files, 256 KiB of patches
and 512 KiB of serialized context, with 1 MiB JSON responses and up to 32 applicable
base instruction paths. These are preparation/transport limits, not investigation
budgets. Oversized preparation is explicitly incomplete; omitted patches and CI
context are disclosed. Workspace source reads support large files through the
existing paged tools (2,000 lines/50 KiB per read). Deletion-only inline anchors,
legacy CI statuses and full CI logs remain limitations. Dependency installation
and test execution are deferred. Quality claims still require labeled evaluation.
The [commit comparison API](https://docs.github.com/en/rest/commits/commits#compare-two-commits)
supplies the merge base, separately from the current base tip. No upstream code
was adapted for this executor.

### Evidence informing the design

Primary documentation inspected on 2026-09-07 informs the following choices.
Product capabilities and vendor-reported quality metrics are not independent
evidence that Renoa has reached equivalent review quality.

| Reference | Relevant behavior | Renoa design consequence |
| --- | --- | --- |
| [CodeRabbit review overview](https://docs.coderabbit.ai/guides/code-review-overview) | Repository context, incremental reviews on subsequent commits, severity categories, and discussion of findings. | Keep per-PR review history; inspect surrounding code; publish concise findings that remain discussable. |
| [Cursor: Building a better Bugbot](https://cursor.com/blog/building-bugbot) | Describes an early multi-pass/validator pipeline, then a move to agentic context gathering; measures findings resolved and evaluates on annotated diffs. | Use agentic investigation and a validation stage. Evaluate actual defects and false positives before multiplying model passes. |
| [Qodo review architecture](https://docs.qodo.ai/code-review) | Specialist review agents with a judge that merges and filters findings; repository history and persistent reviews. | Make investigation and validation replaceable. Retain the evidence and disposition of findings between runs. |
| [Greptile scoped configuration](https://www.greptile.com/docs/code-review/greptile-config) | Directory-scoped rules and explicit context files, with visible configuration precedence. | Apply relevant repository instructions and architecture documents, and show which sources governed a run. |
| [GitHub Copilot code review](https://docs.github.com/en/copilot/concepts/agents/code-review) | Project context gathering, configurable triggers and effort, and handoff of findings to a coding agent. | Keep review effort explicit and make structured results reusable by later fix workflows. |
| [PR-Agent](https://github.com/The-PR-Agent/pr-agent) | A separate community-maintained open-source reviewer with configurable providers and deployment methods; it is not the current hosted Qodo implementation. | A useful inspectable reference, not a replacement Host or proof of parity with Qodo. |

PR-Agent's source was inspected at commit
`782a4e3a6c02189db3ac24240ebe6789629d40c4` (MIT license). No upstream source
has been adapted into Renoa as part of this design.

### Ownership and management

Review Desk is a Host-owned reviewer identity with a review recipe. Repository
subscriptions, trigger policy, frozen run configuration, outcomes, and discussion
context belong to the Host. A temporary review workspace belongs to one admitted
run. The GitHub adapter owns webhook parsing, installation authentication, and
the mapping from Host findings to GitHub reviews and comment identities.

The browser and agent-facing management tools must call the same Host operations.
The initial control panel manages repository selection, automatic review triggers,
draft handling, model/reasoning, limits, pause/resume, manual review requests, and
run/result inspection. Each mutation needs a stable operation identity and a
revision check where edits can conflict; reconnecting must not submit it twice.
The UI must explain the effective configuration and the frozen configuration
used by an existing run rather than silently rewriting history after an edit.

Browser access requires an authenticated management path to the existing shared
Host. The current one-use passkey ticket authenticates an RCP WebSocket only;
it is not a reusable HTTP bearer token. A management transport must bind the
authenticated principal to an explicitly authorized Host and keep credentials
behind that Host. Sharing `renoa.live` must not grant task authority or cause the
coordinator to assemble agents. RCP locked decision 21 permits this separate
management service; it does not implement its authentication or routing.

### Review execution and delivery

1. Validate the GitHub webhook signature over the bounded raw request body.
   Persist the admitted delivery and its logical work identity before returning
   success, within GitHub's response deadline. Duplicate delivery IDs and multiple
   events requesting the same automatic review must not create duplicate work.
   Authenticate installation/repository ownership before admitting expensive work.
2. Default to Renoa's selected repository, non-draft PR creation, ready-for-review,
   and new commits. Support an explicit manual request. Coalesce rapid pushes;
   resolve the current open PR state through GitHub before selecting work, since
   webhook arrival order is not authoritative. Closed PRs and revoked installation
   access cannot continue to publish. GitHub does not automatically retry failed
   webhook deliveries; reconcile retained delivery history against durable Host
   receipts and request redelivery to recover missed work after downtime. This is event-triggered work, not a
   new cron schedule variant.
3. Freeze repository identity, PR, base/head commits, effective policy, model,
   instructions, and tool composition with the admitted run. Reuse its stable
   execution identity during restart recovery. Do not review a moving branch or
   silently substitute a different model after a provider limit.
4. Gather the complete changed-file inventory, relevant diff, surrounding code,
   callers, tests, and available CI results. Explicitly record exclusions,
   truncation, inaccessible files, and budget exhaustion. Repository instructions
   come from the trusted base revision; changes to instructions inside the PR
   are review material, not authority to expand access or suppress the review.
5. Investigate concrete defects and validate candidate findings against the code.
   Each published finding must identify the condition that triggers the problem,
   the consequence, supporting source locations, and a useful correction.
   Reject unsupported assertions, invalid line anchors, duplicate findings, and
   generic style advice. A confidence number generated by the model is not proof.
6. Persist structured findings and a publication intent before calling GitHub.
   Recheck the PR head immediately before publication, suppress known superseded
   work, and always bind the review to its actual commit SHA. GitHub provides no
   atomic compare-head-and-post operation: a concurrent push can still make a
   correctly bound review outdated, and the UI must show that honestly.
7. Retain remote review/comment IDs and reconcile an uncertain post before any
   retry. An unresolvable outcome becomes visible attention-required state;
   it must not cause a blind duplicate comment. Respect API and provider backoff.
   Retry infrastructure stages without rerunning a completed model review.
8. Expose the same durable review result to GitHub replies, the control panel,
   and authorized agent tools. A follow-up conversation must be able to retrieve
   the exact output and evidence even when it starts on a different surface.

GitHub's [webhook guidance](https://docs.github.com/en/webhooks/using-webhooks/best-practices-for-using-webhooks)
requires a prompt response and explains that redelivery retains the delivery ID.
Its [redelivery documentation](https://docs.github.com/en/webhooks/testing-and-troubleshooting-webhooks/redelivering-webhooks)
states that failed deliveries are not automatically redelivered.
Its [review API](https://docs.github.com/en/rest/pulls/reviews#create-a-review-for-a-pull-request)
accepts an explicit commit and inline locations. These support durable admission
and exact-commit publication, not a claim of exactly-once external side effects.

### Composition, resource limits, and review quality

The first reviewer should have repository read/search and bounded CI-context
tools. Its GitHub write authority is exercised by the deterministic publisher,
not exposed as a general model tool. PR text, files, and logs are untrusted input.
The reviewer cannot inherit Arcee's shared accounts merely because they are
available on the Host. Existing filesystem read tools already check workspace
containment. A checkout must not carry Git credentials in its files or config.
Use a GitHub App installation credential owned by the Host and mint short-lived
tokens for the selected repository and required operations. GitHub documents
[installation-scoped tokens](https://docs.github.com/en/apps/creating-github-apps/authenticating-with-a-github-app/authenticating-as-a-github-app-installation)
with a one-hour lifetime and optional repository/permission restrictions. App
registration and installation remain deployment prerequisites; the existing
interactive GitHub MCP connection is not proof that a review App is installed.

`host/runtime.rs` automatically adds `routine_manage` to ordinary `renoa.bot.*`
profiles. The review composer reuses the shared loop and compaction while supplying
only the sandbox's inspection tools. It does not inherit interactive management
capabilities. This is tool composition, not a new popup permission system.

The review composer selects the optional `renoa-code-review` skill from the
Host's existing shared skill catalog. It pins the content-addressed revision and
renders it into the durable review snapshot before inference. PR checkouts are
never searched for this trusted skill. Changes to the shared skill affect new
reviews; replay and compaction keep the frozen instructions. The inspection
tools accept `include_hidden` for configuration and CI paths while retaining
workspace containment checks.

Each investigation and validation stage also uses the existing Host trace store
in `review-sessions/<request-id>/trace.sqlite3`. This records model/tool latency,
first output, reported token/cache usage and provider retries independently of
kernel recovery state. Progress facts must be ordinary assistant text so the
shared compactor can retain them. Responses encrypted reasoning is replayed
unchanged, but its context estimate uses reported output usage instead of treating
ciphertext bytes as prompt text; unknown formats retain the conservative fallback.

Incomplete review outcomes remain in the Host's run and trace records for the
control hub. The publisher settles them as suppressed before requesting GitHub
credentials, so operational failures do not create PR noise. A submission whose
acknowledgement was already lost is still reconciled without reposting. The
current RCP browser console does not yet expose these Host review records; that
management view is part of the control-panel integration.

Worker entry is distinct from the pre-dispatch lifetime record. Failed launches
retry with a persisted backoff inside the original deadline, after confirming
the stable systemd unit is stopped. Partial launch files are then replaced.
Per-job filesystem cleanup failures are retained and retried without preventing
later jobs from progressing. Unknown unit state, a live execution lease and
catalog errors still prevent dispatch; they do not prove that an owner has died.

Begin with one review at a time and explicit working-context/output settings.
Do not impose a review-wide model-call budget. The explicit lifetime is 60 minutes,
with up to 30 minutes for one provider call. Keep review execution from blocking
the existing routine queue.
Keep the system/tool prefix stable, put run-specific metadata after it, and reuse
content by immutable commit/blob identity. Record provider-reported token and
cache usage when available; unknown cache savings or monetary cost stay unknown.
Incremental review should reuse unchanged context and previous findings while
still checking the current PR as a whole. Fall back to full context after a
force-push, changed base, or incompatible review configuration.

A later workspace capability can install dependencies and execute tests.
The current inspection environment uses existing CI evidence and reports that
tests were not executed by the reviewer. Automatic fixes,
cross-repository graph indexing, autonomous rule learning, and multiple parallel
investigators follow a useful measured baseline rather than precede it.

Quality must be tested on fixed base/head pairs with human-labeled defects and
clean changes, including Renoa's real persistence, retry, OAuth, and context bugs.
Separate tuning examples from held-out cases. Track actionable precision,
labeled-defect recall, duplicate/stale findings, missed coverage, latency, token
usage, and cost where known. Resolution at merge is useful feedback, but does
not by itself establish correctness: authors may accept a weak suggestion or
defer a valid defect. Record dismissals and their reasons without automatically
turning arbitrary PR comments into permanent review policy.

Before enabling automatic publication, deterministic boundary tests must cover
signature failure, duplicate and out-of-order events, restart after admission,
rapid pushes, changed heads, revoked access, provider/API backoff, incomplete
diffs, path escape, malformed findings, and lost publication responses. A labeled
review evaluation is separate from these delivery tests; passing Rust tests alone
does not establish that the reviewer finds useful bugs.

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
    the loop, and kernel never own credentials. Callback transport is selected
    once during Host composition and reused by every OAuth path.
13. A headless surface may carry a short-lived encrypted credential-intake
    action, but only the execution Host stores and uses the plaintext. A failed
    setup, authentication, or discovery never publishes a new connection and
    never replaces a working one.
14. Shared package availability is a Host concern. The package registry carries
    immutable package bytes and ordered revisions only; it never becomes RCP,
    remote execution, credential distribution, profile authorization, or
    surface state.

## Open decisions

- future Host schema migrations beyond the proven v1-through-v13 chain;
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

The first hosted surface registers Arcee and maps each allowlisted private
Telegram topic to one caller-identified Host session. It persists an update
before advancing the polling offset, preserves request identity across process
loss, re-drives kernel-owned execution, and never blindly repeats an uncertain
Telegram final send. `/new`, `/compact`, `/status`, `/model`, `/reasoning`,
`/cancel`, native draft
stopping, bounded live drafts, and exact-profile execution cross the real Host
path. Telegram keeps only ingress, topic mapping, and delivery state; it does
not copy Agent history or runtime composition.

The worker registers its request cancellation token before loading an executable
session. A durably cancelled request that has never reached the kernel returns
`Stopped.` without model, MCP, or trace dependencies. Existing sessions are still
checked under kernel ownership: settled outcomes replay, changed content
conflicts, and unfinished work receives a stable durable cancellation request.
Unfinished work still needs its bound runtime to settle through normal recovery;
the Host does not fabricate a terminal result when execution dependencies fail.
Cached sessions apply the same check before preparing execution.

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
semantic receipts. Schema v9 adds durable CIMD, pre-registered-client, and DCR
policy while preserving existing OAuth connections as DCR. New model-initiated
connections select that internal policy from verified metadata. Client credentials
remain named Secret Service references; no kernel or RCP type changed.
Schema v10 adds validated generic credential header names and public prefixes;
existing connection kinds migrate without changing identity or catalog state.
Schema v12 preserves existing loopback flows and adds the alternate callback
relay identity. A real headless Host proof survives interruption, consumes a
durable callback from the coordinator, stores it locally before clearing the
relay, completes exchange, and handles provider rejection by the same rule.
The relay contains no PKCE verifier or token, and no kernel type changed.
Schema v13 lets OAuth attempts exist before an active connection. Encrypted
credential intake, authorization, and discovery therefore complete while the
old connection remains authoritative; one transaction publishes a successful
candidate, while failure publishes nothing.

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

The current MCP adapter is revision v0.8 on process wire 8. Discovery compiles
each external tool's input schema with the pinned SDK validator and isolates an
invalid definition. Invocation validates the exact arguments against the
frozen schema before dispatch. Header credentials remain standard-input-only,
endpoint-scoped, collision-checked, and redacted; older complete catalogs stay
readable but new discovery publishes only v0.8. An unfinished v0.7 adapter
operation fails closed after upgrade instead of being resumed across a changed
wire contract.

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
