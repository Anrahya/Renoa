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
create another loop or history store. Renoa-specific management uses separate
logical Host operations. The planned browser consumer uses the HTTPS boundary
described below; browser, CLI, and model-facing adapters share domain semantics.

The first concrete coding preset is Renoa Alpha v1, specified in
[`renoa-alpha-v1.md`](renoa-alpha-v1.md). Its stable creation preset identity is
`renoa.coding.alpha.v3`. Alpha is one code-owned creation preset, not a special
Host execution type. Arcee is the first personal-operator preset, with stable
identity `renoa.personal.arcee.v3`; Telegram is only its first surface. A
caller-defined agent uses the explicit creation schema; `renoa.general.v1` is an optional general template. Every agent is
one durable, provider-neutral `AgentDefinition`: identity and creation provenance,
an optional creation preset id, the complete core operational document, the exact
tool selection, and the exact selected Host connection ids. Creation snapshots
the preset into that definition; runtime resolution never consults a preset again.

An Arcee agent's stable system rules are part of its stored definition. When
that definition enables the Soul, the Host publishes an owner-editable `SOUL.md`
under `agents/<agent-id>/` in its data directory. The Soul controls the agent's
identity and voice.

`USER.md` belongs to a person, not to an agent. Each RCP principal has one file
at `users/<principal-id>/USER.md`, and every agent that enables the User
document reads the file of the principal whose command started the turn. So two
agents talking to the same person share one profile, and one person never sees
another's. A turn with no principal (a routine, a Telegram or Slack message, the
CLI) has no `USER.md`. A person with no file reads as an empty profile. The
first edit creates the file and its private directory, and an edit rejected for
a stale revision creates nothing.

Both files are read for every newly admitted turn. The `agent_documents` tool
replaces one complete document atomically against the revision shown in the
prompt. A stale edit fails without changing the newer file, which keeps
concurrent edits from two agents safe. Existing files are never overwritten
during startup. Neither file changes kernel state or the surface binding.

The agent learns durable facts during ordinary work and may update either
document through `agent_documents` when the evidence is strong enough. Startup
does not interrogate the user or invent durable user facts from environment
data.

An Arcee agent's stored definition starts automatic compaction when the exact
projected model input reaches 400,000 tokens. The provider's advertised context
window remains unchanged. Compaction targets a 40,000-token rebuilt request
containing the fixed instructions and tools, a bounded checkpoint, and the
latest safe conversation tail. When no safe transcript cut can meet that
target, the planner keeps the smallest safe tail and still respects the real
provider limit. This stored policy follows the agent across surfaces and does
not change Alpha's context policy.

The Arcee preset also stores the OpenCode Go provider restriction in the
agent's own definition. This is agent policy, not Telegram policy: every
surface that opens a session for that agent sees the same provider boundary.
The selected OpenCode Go model and reasoning level remain session configuration
and may change between operations. Host launch configuration may set the
initial reasoning level for new sessions; saved sessions keep their own
acknowledged model and reasoning selection.

The product direction for portable packages, external integrations,
connections, agent selection, and agent-driven capability changes is recorded
in [`renoa-extensions-north-star.md`](renoa-extensions-north-star.md). That
document does not override this Host boundary or prematurely settle the open
v0 storage, permission, and management contracts below.

The first direct external-tool boundary is specified in
[`renoa-mcp-v0.md`](renoa-mcp-v0.md). It resolves MCP tools through the existing
Host and `AgentToolBinding` path rather than adding a parallel runtime.

## What lives where

The same component has three distinct states:

```text
Capability library      installed pieces available to the Host
Agent definition        the durable identity, operational document, exact
                        tool selection, and selected connections of one agent
Resolved runtime        exact bound pieces for one kernel operation
```

The Host owns the capability library, agent definitions, runtime resolution,
provider configuration, credentials, workspace bindings, and future policy. The
kernel owns Agent and Session identity, command admission, operation state,
semantic events, effect safety, checkpoints, and the frozen `RuntimeManifest`.

The kernel stores compatibility identities and revisions for selected pieces.
It does not store or interpret provider, tool, skill, workspace, or product
configuration.

### Reuse within one Host

A Host is identified by its durable data root and Host UUID, not by a surface
process. Several local surface processes may open that same root under the same
OS identity and compatible Renoa build. They share installed plugin revisions,
MCP connection identities and credential resolution, agent connection
selections, and immutable skill files. Each kernel session still has one
exclusive execution owner. An unrelated data root is a separate Host even on
the same machine.

`plugin_manage list` inventories the Host's packages and connections and
reports whether each connection is enabled for the calling agent. `enable`
attaches an existing connection without reinstallation, rediscovery, or another
credential ceremony. Every session of that agent sees the same attachment.
A different agent selects the connection explicitly; availability in the Host
library does not automatically expose every tool to every definition.

To activate an installed package, call `add` with
`source: {"kind": "installed", "package_digest": "<digest from list>"}`.
The Host verifies that exact installed revision and attaches its supported
skills to the agent and restores retained account selections without needing
the original source directory or a network registry. A new MCP connection
remains an explicit separate operation. Whole-plugin `deactivate` and
`enable_plugin` gate future skills and all selected accounts; `replace_plugin`
changes the selected revision with a current-digest check. Missing or corrupt revisions fail rather than
being downloaded or substituted.

Global skills are discoverable by each agent through the configured global
source; workspace skills keep their workspace scope. Loading a skill pins its
exact revision to that session. Sharing the library does not share conversation
history or silently activate another session's instructions. Cross-machine Host
access and credential sharing between distinct Hosts remain separate work.

### Composition and management boundaries

The personal-system direction below guides the control-panel implementation.
Authenticated browser observation and owner routine enablement exist; general
agent editing and delegation remain future work. Host ownership is a logical boundary, not a requirement that one
object, executable, or crate implement every subsystem. A laptop and a VPS are
deployment choices. Preserving a Host across machine replacement requires its
durable identity, records, and credential material; a hostname is not its identity.

Keep responsibility with the component that implements the behavior:

- Identity authenticates a caller. Host authorization binds that caller to an
  exact Host and allowed operations. Neither component runs agents.
- Host composition selects existing provider, loop, context, tool, credential,
  workspace, and execution components. Their implementations remain outside the
  management transport, and provider and surface policy remain outside the kernel.
- Domain operations own their validation, transactions, and durable receipts.
  Routines retain scheduling semantics. A management router delegates to those operations rather than reimplementing them.
- Observation projects committed metadata independently of runtime construction.
  Inspection and configuration changes that do not execute work must not require
  model discovery, valid provider credentials, or acquisition of session ownership.
- Browser, CLI, and model-facing tools adapt the same application operations.
  They do not access each other's UI or own alternate scheduling or agent stores.
  RCP continues to own task continuity; its coordinator does not depend on the
  local Host implementation to assemble agents or interpret Host inventory.

Use concrete modules in the current implementation. Extract a component when a
real consumer or enforceable dependency boundary requires it; do not introduce a
generic service bus, universal component trait, or a second competing Host. One
deployment may compose multiple modules in one process. Process separation is
needed where execution or credential isolation requires it, not merely to make a
module appear independent.

Host management is a built-in capability selectable for an agent, not a special
kind of agent or a dependency on shell access. Its tools bind caller identity in
the trusted runtime; a model-supplied agent ID is never proof of authority. The
CLI is another optional adapter and may be run on the Host machine or, after
device enrollment is implemented, remotely. The owner identity exists before
any agent. A personal operator receives explicit management authority and can
be replaced; being the first agent or creating a child grants no implicit root
authority. Human management must not impersonate Arcee to reuse an agent API.

Future agent editing selects supported installed components and persists that
selection before runtime assembly. Installing a capability, selecting it for an
agent, and resolving it for an operation remain separate. A surface binding is
optional and independent of the definition: schedules or delegated work may
target an agent without a chat channel. This direction does not promise
arbitrary hot loading of Rust implementations or make existing built-in presets
editable. Future delegation needs durable assignments and result references
outside the orchestrator's transcript. Add those contracts only with their
execution consumer.

### Personal control-panel boundary

The first browser consumer targets the existing shared Host through a separate
same-origin HTTPS management API. Reuse direct browser pairing and passkey verification while keeping the
management session separate from RCP's one-use WebSocket tickets. Bind the verified
principal to the configured Host explicitly. Management sessions must be revocable;
cookies must be secure and HTTP-only, and state changes must check request origin
and protect against cross-site request forgery. Identity code may be factored for
the two real consumers, but the coordinator must not acquire a dependency on
Host runtime construction. A browser-provided Host ID, path, or actor ID cannot
choose another data root or substitute an authenticated identity.

The initial overview uses `HostObserver`. It shows recorded work, attention, schedules,
agents, and installed capabilities with progressive disclosure. It must preserve
the observation qualifications below, including unknown worker liveness. Display refresh time and stale/error
states. Start with refreshable HTTP snapshots; do not add another event journal
or infer an event sequence from differences between snapshots. Surface health,
effective runtime tools, and other details need their own evidence before display.

`renoa-management` serves the built panel and the metadata API independently of
execution workers. Its configured loopback identity service validates the browser
cookie; the adapter does not read the identity database or execute agent turns.
With model configuration it starts catalog/describe requests to validate creation.
`GET /v1/host/access` reveals only the public owner login identifier. `GET /v1/host`
requires that authenticated owner. The binary pins one existing Host UUID and refuses
a replaced Host even at the same storage path.

The browser retries unavailable services and network failures, keeping the last
snapshot explicitly stale. Only rejected credentials clear the view. Its remembered
session lives in identity storage, with a secure HTTP-only same-site cookie and a
180-day lifetime renewed during use; neither source IP nor a process-local secret
binds the login. The lifetime is not a promise of permanent access: explicit
revocation, cookie deletion, expiry or loss of identity storage requires signing in.
Different browsers enroll/sign in once each. Slack and other native surfaces keep
their existing credentials; they do not run browser passkey ceremonies.
Direct Host-code pairing is the default for browsers without a passkey provider;
passkeys remain optional. Both authenticate the same configured human owner. Their
admission and retry rules live in [identity-v0.md](identity-v0.md), independently
of Host assembly and surface adapters.

`HostRoutineControl` exposes owner pause/resume without constructing an execution
Host. `POST /v1/host/routines/{routine_id}/enabled` adapts that domain operation;
the browser exposes it beside the schedule on both Work and agent detail. The trusted adapter supplies the
authenticated principal separately from JSON input. Each write requires the
configured owner cookie and exactly one matching `Origin` header. The management
configuration names `public_origin`; forwarded headers cannot select it.

The JSON request contains `operation_id` (a fresh UUID for each logical change),
`expected_revision`, and the desired `enabled` boolean. The server rejects unknown
fields and bodies over 4 KiB. It persists the routine change and owner receipt in
one transaction before acknowledging. An identical retry, including after restart,
returns the original receipt even if another edit has since occurred. Reusing an
operation ID with different input or applying a stale revision returns 409. The
response contains the operation ID, routine ID, committed revision, enabled state,
and next due time; it excludes the standing prompt. Read a new Host snapshot for
current state rather than treating a historical receipt as the latest revision.

The routine domain retains scheduling semantics and agent restrictions. Pausing
prevents future admissions, not completion of already-admitted work. Resuming an
interval starts its next period from the resume time; daily schedules choose the
next occurrence in their named timezone. Resuming an expired one-time schedule
returns 422 and needs a new future date through the existing agent editing path.
Deleted routines return 404; replaying an older receipt cannot restore them.
Host identity is checked inside the mutation transaction, including on replay.
Owner receipts remain distinct from agent receipts and cannot grant agent tools
owner authority. Definition editing and delegation remain separate from this operation.

### Owner creation and Discord channels

`GET /v1/host/agents/options` returns enabled provider models, reasoning choices,
Host defaults, and selectable native tools. `POST /v1/host/agents` accepts the
canonical `AgentCreateRequest` (snake_case JSON, 64 KiB transport ceiling). The
adapter supplies `Principal` provenance and `Management` origin from the verified
owner; the body cannot supply them. Creation validates before writing and replays
its stable operation receipt after restart without another provider lookup.
`GET /v1/host/agents/{id}` returns the owner's saved definition. The browser keeps
uncertain requests in tab storage and retries their exact operation ID and fields.
Native grants start empty; plugin discovery, management, and invocation are universal.

`documents.user` decides whether the new agent reads the `USER.md` of whoever talks
to it. So that the owner can make that choice knowingly, `GET /v1/host/profile`
returns the signed-in owner's own profile as `{content, revision}`. An absent
profile is empty content with the revision of empty content. `PUT
/v1/host/profile` takes `{expected_revision, content}`, checks the origin, and
makes the same revision-checked edit as `agent_documents`. A stale revision returns
409 and writes nothing, even on a first save, and the response is the new profile.

`GET /v1/host/discord` reports `setup_required` until the owner connects a bot,
then `connected` with the bot and server names, the default agent, and saved
bindings. Connection is not worker liveness. `POST /v1/host/discord/inspection`
checks a pasted token without saving it: the token must be a bot whose application
has Message Content Intent; it returns the bot name, an Administrator invite link
and the bot's servers, each marked with whether its roles grant Administrator.
`POST /v1/host/discord/connection` accepts `operation_id`, `bot_token`, `guild_id`
and `agent_id`. The adapter verifies the Host agent, then re-reads Discord and
requires Administrator in the chosen server before committing one owner-only
`credentials/discord.json` by hard link. The record holds the token, server,
application owner (the operator) and default agent; HTTP responses never return
the token. An exact retry returns the committed connection before contacting
Discord; any other request is rejected, so the connection is set once. The
browser keeps the token in memory only. `GET /v1/host/discord/channels` lists the
connected server's text and announcement channels in sidebar order.
`POST /v1/host/discord/bindings` accepts `operation_id`, `channel_id`,
`agent_id`, and `expected_revision`. The adapter verifies the existing Host agent
and Discord text/announcement channel in the connected guild before storage.
It does not probe send permission. Revision checks, binding and receipt commit
in one SQLite transaction; stale edits and reused operation IDs return 409.
An exact retry returns its original receipt before contacting Discord again.
Every write requires the owner cookie and exact Origin.

Discord owns `state/surfaces/discord/discord.sqlite3` (schema 5). The worker
runs no agents: each channel's conversation is an RCP task on its routed agent's
target, `agent:<uuid>`, opened on the node that advertises it. Channel routing,
the task, and a stable command identity are persisted when a message is
admitted, and the message is submitted under that identity, so a reconnect
retries it exactly. Reassignment starts a new task for subsequent messages;
already admitted work keeps its original task. Unbound channels still require a
mention, reply, or active thread, and only the operator (the application owner)
can use DMs. Task records apply once under a per-task cursor; each finished
command becomes one reply, including commands submitted to the task from another
surface. A message whose agent has no online node is answered as not sent. The
launch file holds only the home and the RCP endpoint and credential; the guild,
operator, default agent and token come from the committed connection, which the
surface database then pins.

The executing node consumes structured `plugin_manage` progress. OAuth and
credential setup links are delivered only to the operator's DM through the
Host's Discord connection, with mentions and embeds disabled, and never enter
the RCP task journal. SQLite stores a digest and delivery state, keyed by the
RCP command, never the link. Confirmed rate limits may retry; an unknown send
outcome stops that command as failed and requires checking the DM before
restarting. Recovery never blindly repeats an uncertain link.

### Consistent management

One configured human owner controls one durable Host. The panel has three entries:
Work for current attention, unfinished work and upcoming schedules; Agents for
the records that own that work and their applicable controls; and Shared library
for installed connections, plugins and recorded skills. Shared connections link
back to the agents whose definitions select them. Similar names do not justify merging
agent identities, and a surface process is not a separate human owner.

Management is composition of domain operations, not a second execution system.
The HTTP adapter authenticates the owner, validates the request origin and adapts
typed requests; domain modules retain their transactions and rules.
Human operations do not impersonate an agent. Agent tools bind their own actor in
the trusted runtime. They share domain rules with owner operations without gaining
owner authority. Adding a future definition editor, binding editor or management tool
must follow this boundary rather than introduce another store or an HTTP-only rule.

The browser persists a pending operation's identity and configuration-only body
before sending it. Lost responses survive navigation and reload; retry sends that
same body. Confirmed receipts cause a fresh observation read, never replacement of
current state with an old receipt. Definite revision conflicts require reviewing
the latest record. Configuration controls are unavailable on stale snapshots; a
network outage does not create a new login requirement. These pending records do
not contain cookies, credentials, standing prompts or conversation content.

Recorded operation state does not establish worker liveness. Changing a schedule
is not cancelling its agent.

These controls establish a consistent management path, not general agent assembly.
Owner creation and editing of definitions, surface-binding controls, runtime capability
resolution and durable delegation remain future consumers. The existing Host remains
their composition point; RCP's continuity contracts do not absorb product policy.

## Agent identity and assembly

### Personal Host observation

The control panel targets one person's existing Host identity. Its agents,
installed capabilities and automation records belong to that Host;
surface processes are clients of those records. One logical Host does not require
one process, and a second data root is not implicitly part of the same Host.

`HostObserver::open` opens an existing compatible data root and pins its Host UUID.
`snapshot` reads agent identities, ordinary session operation summaries, routines,
shared connection selections and recorded plugin/skill revisions.
`renoa-host inspect <data-directory>` is the first consumer. It requires
OS read access, not a launch configuration, model provider, adapter or credentials.
It neither initializes/migrates a Host nor repairs or imports legacy records.

Catalog facts come from one read transaction. Each session has its own subsequent
read transaction; the response is not a globally atomic view across databases.
Missing/corrupt sessions are reported individually. Catalog failure fails the
snapshot, and replacing the Host UUID requires explicit reconnection. Legacy
session identities are projected without writing them into the catalog.

The kernel's non-owning observation API supplies committed operation state without
loading command bodies, checkpoints, effect payloads or transcripts. An unfinished
operation is not proof of a live worker. A stored MCP catalog is not a connection
health probe. Agent connection selections are not a claim about the frozen tools
of an already-admitted operation. Recorded skills are not necessarily loaded in
any session. Large
artifacts, instructions, provider diagnostics and credential material stay outside
the overview response and need separate, deliberate detail reads.

This is local observation, not a browser authentication or management transport.
The browser path must authenticate its principal against this exact Host before
exposing inventory or accepting operations. It must not reuse an RCP one-use
connection ticket as an HTTP bearer token. RCP remains responsible for continuity
and durable delivery; Host management remains a separate application boundary.

An Agent Instance is durable identity and isolated history. It is not the
temporary collection of Rust objects used to execute one operation.

For each operation, the Host conceptually resolves:

```text
Agent Instance + stored definition + installed capabilities + current scope
                              |
                              v
                    exact resolved Runtime
```

The kernel freezes that runtime before execution. A later definition or component
change cannot replace its implementations. The fixed extension registry is an
explicit exception for data visibility: it may read later committed Host state,
but every executable reference is catalog-bound and fails stale rather than
changing behavior.

Agent definitions are durable Host records. They do not execute effects and do not
own session history. Installed availability does not imply future authorization;
that distinction remains required even though v0 intentionally has no
permission system.

Creation presets are code-owned, versioned, immutable seeds: a stable validated
identity, base instructions, an optional exact model-provider restriction, the
workspace-instruction and turn-timing policy, an optional document set, and an
optional automatic-compaction policy. Creation copies the applicable values into
the agent's own operational document, so a preset change requires a new preset id
and never rewrites an existing agent. A session manifest persists the exact agent
id; Host catalog storage holds the definition. Loading fails closed when the agent
definition is absent or corrupt. Caller-defined agents store their own
instructions, and every agent definition is stored in `host.sqlite3` and resolved
by already-running Host processes.

The Host catalog stores one stable Host UUID and the canonical agent tables: one
`host_agents` row owning identity, creation provenance, the optional preset id,
and the complete operational document, plus normalized rows for the exact tool
selection (`host_agent_tool_selections`) and selected connections
(`host_agent_mcp_connections`). Typed receipts make creation, rename, and
tool-selection edits idempotent. `LocalHost::create_agent(creator, origin,
request, cancellation)` is the one creation operation: the agent id derives from
the operation id (`derived_agent_id`), so a retried call returns the same agent
while a changed request under the same operation id conflicts. Trusted
actor/origin pairs are enforced: an agent-tool caller is `AgentCreator::Agent`
with `AgentCreationOrigin::AgentTool`; owner management is `Principal` with
`Management`; trusted provisioning is `System` with `Provisioning`.
`ensure_agent_session` binds an exact caller-supplied Session ID to an existing
Agent. Several sessions may share that Agent while retaining independent
workspaces, model selections, histories, and exclusive execution ownership.
Deleting a session does not delete its Agent definition.

A session manifest is `{version: 4, agent_id, session_id, workspace}`. Published
manifests remain authoritative for their Agent, Session, and workspace binding;
the agent must already exist when the session is created, and a manifest naming a
missing or mismatched agent fails closed. Creation commits the agent definition
before any session can bind to it, so a crash between the two never invents
another Agent. The catalog records identity and composition selection, not a
second execution journal. Kernel operation facts remain the execution authority.
The [Slack adapter](../crates/renoa-slack/README.md) consumes the durable agent
management path: its configured Arcee Agent survives restarts, while DMs and
channel threads bind independent conversations through `ensure_agent_session`.
Its transport admission and reply receipts remain surface-owned. Slack verifies
that its configured agent id is already provisioned and no surface can create a
`host_agents` row; routines use the same Host identities and execute
independently of surfaces.

Telegram, Slack, WhatsApp, ACP, a GitHub webhook, and a GUI are surfaces or ingress
adapters; they do not become agents merely because they deliver messages. A
daily-assistant agent definition may be used from any compatible surface.

## Capability composition

Every agent receives `plugin_search`, `plugin_manage`, and one invocation tool:
`code_mode` when the Host configures the pinned Monty worker, otherwise
`tool_execute`. These are platform capabilities and cannot be removed through
machine-tool selections. Targeted discovery comes before `*` browsing. Plugin
cards and nested tools retain the 200-item limit; exact references return the
full model-facing input schema.

The exact machine-tool selection contains only:

```text
read_file
edit_file
write_file
bash
grep
find
```

An explicit creation selection replaces the optional template's tools, including
`tools: []`. Without a template or a selection, a new agent has no machine tools.
Alpha and Arcee templates default to all six. Every agent still has discovery,
plugin management, and invocation. The agent creation plugin may select any
machine tools for a child; it exposes no operation to grant tools to its caller.
`set_agent_tools` and `renoa-host <config.json> agent-tools <edit.json>` are trusted
owner operations. Their revision-checked receipts remain independent of the
original creation receipt. Active operations retain their frozen grants.

Five compiled Host plugins use the same discovery, exact-schema lookup, and
invocation boundary as external MCP tools:

| Plugin | Capabilities |
| --- | --- |
| `renoa.agents` | Create, list, rename agents |
| `renoa.routines` | Manage schedules and read retained results |
| `renoa.documents` | Edit this agent's SOUL and the speaking person's USER file |
| `renoa.skills` | Discover skills and activate exact instruction revisions |
| `renoa.git` | Inspect local changes, diffs, and commits |

These plugins start enabled; document tools exist only for definitions that enable
documents, and `USER.md` is editable only in a turn that names a principal. `plugin_manage` can enable or deactivate a compiled plugin for its
caller. Stored state is checked again on discovery and dispatch. Imported
manifests cannot register native implementations or change machine grants.
References bind the real schema and implementation revision; stale references
fail and require a fresh search. External packages continue to use immutable
content identities and Host-reviewed provider families.

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

Revision-checked file edits and agent-document updates coordinate cooperating
Renoa writers, including separate processes, with a persistent
`.renoa-lock-<digest>` sidecar in the target's canonical parent directory. The
OS lock covers revision
validation, replacement, and parent-directory sync; its empty sidecar must not
be deleted while writers may be active. Process exit releases ownership.
Distinct replacements based on one revision cannot both succeed. Identical
document retries still succeed. Unconditional `write_file` also takes the lock
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
the stored agent definition, exact session/Agent identity, canonical workspace binding,
and authoritative data integrity. It cannot execute or recover a turn; callers
drop the handle and use normal executable loading after repairing dependencies.
ACP uses this path when normal session loading is unavailable.

`state/host.sqlite3` schema v32 keeps Host identity, the canonical agent definition
tables (`host_agents`, `host_agent_tool_selections`, `host_agent_mcp_connections`,
`host_agent_creations`, `host_agent_tool_selection_operations`,
`host_agent_renames`), installed package metadata, supported package MCP entries,
direct integration and connection identities, non-secret credential references,
durable non-secret OAuth phases and terminal receipts, complete MCP catalog
snapshots, per-agent selected connection identities, immutable skill revisions,
agent skill bindings and rejections, session skill activations, routines
and their results and receipts, and authenticated-owner routine receipts.
Registration, discovery, and agent connection selection remain separate states.
Catalog replacement and selection are transactional, and multi-query reads use
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
keeps its own credentials, MCP connections and discovered catalogs, agent
connection selections, session skill activations, workspaces, and session history. The
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

The Host always offers the plugin protocol described above. Targeted search
returns compact plugin cards and up to three matching Host or MCP tools. A small
preview schema is complete; otherwise an exact reference request returns the
full schema within 64 KiB. Plugin cards and nested tool pages return at most
200 items within 50 KiB. A missing MCP adapter fails remote execution visibly;
it does not prevent Host plugins from running.

`plugin_search` with `plugin` and `query` searches that plugin's enabled tools.
For an external MCP plugin, this includes its enabled accounts and endpoints;
each returned reference identifies its connection. `connection` and `query`
select one account or endpoint. An external plugin id without `query` returns
component and connection facts.

A configured exact-pinned Monty worker automatically replaces the visible
`tool_execute` with `code_mode`, retaining the same executor as a hidden durable
binding. Python uses `await plugin(reference, arguments)` for either Host or
MCP tools. No MCP adapter is required to invoke a Host plugin. Every nested
invocation remains independently durable; the model sees the final Python value.
The tool guides the model to check error flags, use supported pagination and
field filters, and return only the task's needed fields from large results.
The evaluator pool starts empty and is capped at two subprocesses. Worker hash
validation happens before mutable Host state is opened. This release's worker
is Linux x86-64 only; an invalid configured worker refuses startup.

The discovery and MCP tools open current `host.sqlite3` state for each call. A committed
connection attachment or catalog refresh is therefore visible on the next
search even when the surface process, Agent session, and current turn are
already running. The kernel freezes their bindings, while exact references prevent a newer catalog from
silently changing a selected invocation.

The Host offers one fixed `plugin_manage` tool. Its model-facing schema is
flat and uses only the broadly supported JSON Schema subset needed by
OpenAI-compatible providers. The Host still decodes one exact, closed variant
for each typed action and rejects missing or cross-action fields:
add one MCP definition independently
verified against the provider's official documentation or one content-bound
local Agent Plugins 1.0 directory; inspect a local package; install the exact
inspected digest; list package integrity and durable connection state; connect
one supported package MCP server for the calling agent, optionally carrying
an exact scope from a prior `oauth_insufficient_scope` result; authorize,
scope-upgrade, or explicitly restart one registered OAuth connection;
disconnect one connection from that agent without deleting its durable
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
server's exact validated scope. `plugin_manage` unions that scope with the
stored grant and opens fresh consent when it widens permission. It never
silently retries the denied MCP call; the Agent must authorize and then issue
one explicit retry. Registration modes are not model input or fallbacks to
guess: strict endpoint metadata must name one issuer; the Host then chooses an
existing issuer-bound client, DCR when advertised, hosted CIMD when advertised,
or a developer-console client form already bound to that issuer. The
headless setup form's frozen wire spelling is `oauth_client`; coordinators also
accept the short-lived buggy `o_auth_client` spelling only for rolling upgrade
compatibility.
The Host discovers and attaches through the same MCP catalog path used by
`LocalHost`; the next `plugin_search` sees the connection
without restarting the session or surface. Disconnect is idempotent and the
next search stops exposing its tools while the verified catalog remains
available for recovery or later reattachment. Package skills enter the same
skill registry under a lower-priority plugin scope; workspace overrides global, and
global overrides plugin. Plugin skill sources use stable per-agent activation
identities; equal package names never replace each other. An equal skill name
from another enabled plugin produces a component rejection. Explicit replacement
keeps the activation identity and uses the current digest to reject races.
The next `skill_search` sees a committed package skill without restarting the
session or surface. Model-facing management results use the same 50 KiB
tool-output boundary as local tools and fail instead of silently truncating
package facts. List keeps aggregate state below that boundary by returning at
most 200 compact package, server, notice, connection, and skill facts per page.
Its opaque cursor is bound to the complete inventory revision, so concurrent
changes produce a visible conflict and a fresh first-page requirement rather
than offset drift. Package integrity, durable connection state, agent
selection, and accepted/rejected plugin skill bindings remain separate facts.
Revision 12 freezes encrypted credential intake and transactional connection
publication. An unfinished revision-11 management call fails closed after
upgrade instead of resuming across the changed effect boundary.

The Host also offers exactly two Agent Skills tools: `skill_search` and
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
Host resolves the agent's stored definition before resolving these inputs:

- provider and model;
- reasoning configuration;
- the agent's stored instructions, optional bounded workspace `AGENTS.md`, and
  exact active skill instructions; and
- exactly the six possible machine grants plus the fixed plugin protocol;
  management, documents, skills, and Git inspection are discovered plugin tools.

`build_local_runtime` resolves that definition with a `LocalWorkspace`:

```text
LocalRuntimeConfig + stored AgentDefinition
  + BridgeModel
  + CompactingContextStrategy
  + LocalWorkspace tools
  + Host MCP, plugin manager, and skill registry tools
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

Model and reasoning selection are not agent identity. They may change
between operations while the Agent Instance, Session, instructions, tools, and
history remain continuous. A change never mutates an active operation; the
kernel freezes each operation's exact model and reasoning revision.
An agent definition may restrict which configured providers are eligible
without fixing a particular model. Discovery, loading, and later model changes
all enforce the same restriction.

The Host resolves a fresh runtime for every newly admitted operation. This
re-reads the canonical workspace `AGENTS.md`, so a project-rule edit applies to
the next turn without restarting the surface. It cannot change an operation
that is already admitted because the kernel has frozen that operation's
manifest.

The selected agent identity is durable session state, and the definition itself
is durable Host state in `host.sqlite3`. Capability edits are revision-checked
and receipt-backed; editing instructions or connection selections remains future
work until a consumer proves that mutation contract.

## Command path

Local management commands use the same typed Host operations as future
surface and model-facing callers, and emit JSON:

```sh
renoa-host inspect <data-directory>
renoa-host <config.json> provision <provision.json>
renoa-host <config.json> agent-tools <edit.json>
renoa-host <config.json> reset <backup-directory>
renoa-host <config.json> rename-agent <agent-id> <expected-name> <name> <operation-id>
```

`inspect` opens the existing data root read-only and prints the Host snapshot.
The remaining commands assemble the ordinary Host from the same launch
configuration a running service uses. `provision` is the trusted creation path
for a `System`/`Provisioning` caller: the first agent on an empty Host is created
by it. Its document is the canonical creation request in camelCase JSON, for
example `{"operationId":"<uuid>","presetId":"renoa.coding.alpha.v3","name":"Alpha"}`,
with optional `instructions`, `tools`, `connections`, and `routine`. `agent-tools`
applies one revision-checked capability edit. `rename-agent` applies one
expected-current-name-checked display-name edit with an explicit operation id. `reset` is the
bounded clean-break reset described below. None of these commands needs an
executable model bridge.

The ACP adapter's `renoa-agent agents list`, `renoa-agent agents show AGENT_UUID`,
and `renoa-agent agents session AGENT_UUID SESSION_UUID /absolute/workspace`
commands remain optional adapters over the same reads and session binding. They
use `RENOA_HOME`, `RENOA_AGENT_ID`, and the provider launch settings. Listing
and lookup need no executable model bridge; session creation resolves the stored
definition and model normally, and the returned Session ID can be reopened
through the existing ACP load path. Host and Agent identity, creation retries,
creator links, and isolated
multi-session execution are tested across restart. The Host UUID identifies a
durable data root; this slice does not replicate catalogs across machines.

An agent adds GitHub or another external MCP through `plugin_manage`, using
the same researched `source.kind=mcp` contract. Endpoint and public headers come
from provider documentation; credentials use an owner-provisioned secret
reference or the supported OAuth setup. A successful add installs the immutable
package, activates it for the caller, and publishes its catalog. The next
`plugin_search` sees the connection without restarting the agent. MCP schemas
are returned by targeted search or exact-reference lookup.

The first real Host flow accepts either an ordinary prompt or a typed compact
control:

```text
surface adapter or local caller
  -> LocalHost loads or creates an AgentSession for an existing agent
  -> AgentSession accepts one caller-identified command
       -> read current workspace rules
       -> resolve the stored definition, model, context, loop, and tools
       -> LocalSession atomically admits the command
       -> drive that exact operation through the kernel
       -> project a durable assistant or compaction result
```

`Kernel::submit_exclusive` combines the unfinished-operation check and command
insert in one immediate SQLite transaction. `AgentSession` uses this optional
admission primitive because one conversation turn must finish before another begins.
The general kernel `submit` path still permits ordered queues for other
compositions. Exact redelivery remains idempotent; a different command is rejected
without leaving ghost queued work.

`LocalSession` remains the lower shared command boundary used by Host agents
and the headless diagnostic runner. Its prompt and explicit-compaction methods share
the same exclusive admission, stable command identity, drive, cancellation,
and durable replay path. `LocalTurnOutcome::Compacted` carries the persisted
post-compaction input estimate without pretending that a control operation
produced an assistant message. `AgentSession` is the complete surface-facing
Host boundary: it also owns runtime selection, persistence, fresh per-turn
composition, and cancellation coordination.

Agent definitions may opt into Host turn timing. A direct caller observes the Host clock
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

`LocalHost::ensure_agent_session` accepts a caller-chosen UUID for durable surface
admission. Repeating it resolves the already-published session with its stored
model selection, Agent identity, and workspace instead of validating a
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
reasoning. Its effect binding is never-replay, so the kernel cannot repeat it
either. If the process dies before that result is persisted, the kernel's
conservative `OutcomeUnknown` recovery boundary still applies; the kernel
never invents an uncertain result, and a safe-to-replay effect's live unknown
report from its first durable dispatch is replayed once before that outcome
becomes durable, unless a cancellation is already recorded for the operation.

Local Host state uses one installation home. Resolution is an explicit launch
`home`, then `RENOA_HOME`, then `~/.renoa`; empty overrides are skipped. All
launchers use the same layout; service installations may supply an absolute
home. Managed roots and agent workspace ancestors reject symbolic links.
Newly created directories are private
on Unix. Explicit credential files, authenticated CLI stores, binaries, package
sources, and surface workspaces may be owner-supplied external references.
If directory creation fails, initialization attempts every newly created
directory in reverse order. A cleanup failure preserves the creation error as
the cause and reports the directories it could not remove; it never deletes
another writer's files to empty a directory.

```text
~/.renoa/
  config/                         launch configuration
  credentials/
    models.sqlite3                default model credential-store location
    discord.json                  owner-only Discord bot connection, set once
    oauth-secrets/<sha256>.json    private remote OAuth/API-key secrets
  plugins/<sha256>/                immutable external plugin packages
  agents/<agent-id>/
    SOUL.md                        enabled identity document
    workspace/                     scheduled/headless agent workspace
  users/<principal-id>/
    USER.md                        one person's profile, shared by their agents
  state/
    host.sqlite3                   definitions, plugins, connections, receipts
    skills/<sha256>/               imported immutable skill revisions
    oauth-locks/                   process-safe per-connection locks
    credential-relay-state/        private pending encrypted-intake identities
    shared-registry/               transient package transfers
    surfaces/<surface>/            surface-owned queues and delivery state
    node.sqlite3                   node continuity state
    coordinator.sqlite3            coordinator state
    registry/                      private package-registry state
  sessions/<session-uuid>/
    session.json                   agent/workspace binding
    runtime.jsonl                  provider/model/reasoning selections
    kernel.sqlite3                 execution and recovery truth
    trace.sqlite3                  diagnostics
  runtime/                         owner-installed runtime assets
```

A component creates only its own stores. Directory initialization does not
configure accounts, install binaries, or create an authenticated model store.

Usage, cache counts, execution timings, provider payloads, streamed chunks, and
tool diagnostics belong in `trace.sqlite3`, never `runtime.jsonl` or model
context. The admitted user-turn observation described above is the narrow
exception: it is semantic model context, not diagnostic trace timing.
Trace rows explain execution but never decide replay or semantic history. A
trace database is owned by one exact Agent and Session; a mismatched or
unsupported trace schema fails closed instead of being reinterpreted. The clean
break below does not migrate old session or trace stores.

The Host assembles these files in a hidden directory. After all four are synced
and the kernel lease is closed, it atomically renames that directory to the
session UUID and syncs the parent directory. Initialization failure removes the
staging directory, so a loadable session is never partially published. On Unix,
the published session directory is owner-only because trace and history contain
prompts, source text, and tool data.
The global `state/host.sqlite3`, `plugins/`, and `state/skills/` stores are owner-only on Unix.
`runtime.jsonl` recovery truncates an incomplete crash tail before any later
append; future valid records can never be joined onto torn JSON.

## Clean-break deployment and reset

Renoa opens only the canonical home layout. Root-level legacy catalogs and
previous directory layouts are not imported or opened; launch files use `home`.
An incompatible catalog found at `state/host.sqlite3` fails closed with an
explicit reset instruction. The canonical agent definition replaces the earlier
profile and bot records rather than reading both shapes. Ordinary startup never
deletes broad filesystem state.

Catalogs at the canonical database path with schema 28–31 upgrade to schema 33
by retaining exact machine grants, removing former Host and plugin protocol
tool selections, and dropping the retired GitHub review tables. A schema 32
catalog already holds current selections and exact plugin activations, so its
upgrade only drops the review tables. Live selections
and creation, rename, and selection receipt results advance one revision when
their grants change. Agent identities and
operational definitions stay intact; Host plugins use their activation state.
Unknown tool names or revision overflow during conversion reject the transaction with reset
guidance. Reopening the upgraded catalog does not repeat the conversion.

1. Stop every writer: the routine service (`renoa-host <config.json>`), every
   surface, and every node daemon that owns the data root.
   A copied data root must not have a live writer.
2. Create exactly one consolidated backup of the previous release's data root.
   `renoa-host <config.json> reset <backup-directory>` does this FIRST: it copies
   the whole data root to a fresh backup directory, refuses a non-empty backup
   directory, and refuses a backup inside the data root. The copy completes
   before the cutover touches the root.
3. Apply the bounded reset, which performs the schema cutover. An earlier data
   catalog at the canonical path (any version below 28) is refused until this runs, so
   the reset is the only path that changes those tables. It drops the retired
   agent-owned tables (`host_agents` in its old shape, `host_bots*`,
   `profile_mcp_connections`, `profile_mcp_tools`, `profile_skill_bindings`,
   `skill_source_rejections`, the agent skill bindings, the routine records tied
   to those agents, and the retired GitHub review tables) and recreates the
   canonical `host_agents` root and its normalized children in their current
   shape, running the earlier migration ladder first for the shared domains it
   still owns.
   `renoa-local/src/host/reset.rs` owns the bounded reset: it removes
   agent-owned rows, the session directories, enabled SOUL files, and
   predecessor document roots. It preserves canonical
   `agents/<agent-id>/workspace` directories and people's `users/` profiles,
   and never deletes workspace files or the Host's shared state. One transaction
   deletes the whole row set with foreign keys deferred, so the order it is
   written in cannot break a reset, and a test that classifies every catalog
   table is what keeps the delete list complete.
   Host identity, MCP catalogs, connections, authorizations and credentials,
   installed plugins, skill revisions, shared registry state, and provider
   credentials are preserved. Schema cutover and canonical agent-row clearing
   share one transaction, so a catalog failure leaves the database unchanged.
   Applying the reset twice is safe. A reset that is
   refused before it deletes anything — a managed root that is a symbolic link
   or another file in place of a directory, or a catalog failure — leaves the
   database untouched; a failure while removing directory contents can leave
   the agent-owned rows deleted and the managed roots partly cleared, and
   re-running the reset converges because both removals are idempotent.
4. Provision the configured bootstrap agent. `renoa-host <config.json> provision
   <provision.json>` performs the one canonical creation operation with a
   trusted `System`/`Provisioning` actor, so an empty Host gets its first agent
   without a model call. Surfaces never create agents; they receive explicit
   agent ids.
5. Start and health-check the current runtime. The Host fails closed when the
   schema or Host identity is incompatible, and each surface fails closed rather
   than creating an agent when its configured agent id is absent. Confirm the
   roster and shared inventory with `renoa-host inspect <data-directory>`, or
   the authenticated `GET /v1/host` snapshot.
6. Only after health succeeds, replace the older release backup. Retain the
   single consolidated backup described in step 2 and delete older release
   archives, snapshots, and unused versioned binaries.

The stores are separate databases with no cross-store transaction. The Host
reset is one idempotent step over `state/host.sqlite3` and the Host session
directories. The node ledger is a separate idempotent step over
`host_node_metadata`, `host_node_tasks`, `host_node_executions`, and
`host_node_events`; this release renames the task's agent column and refuses an
earlier ledger by name. Stop the node service, take the consolidated backup,
delete only `<renoa-home>/state/node.sqlite3`, and apply the canonical Host reset
to `<renoa-home>`, using a fresh subdirectory inside that consolidated
backup. The node runs in the shared Host, whose plugins, MCP state, skills and
shared registry are mutable shared state, not derivable node configuration.
Keep `<renoa-home>/credentials/models.sqlite3`, then start the daemon again.
Each surface store is its own step: the Slack store owns
`identity`, `sessions`, `conversations`, `requests`, `messages`, `receipts`,
`deliveries`, `bot_channels`, `bot_channel_labels`, `setup_actions`,
`routine_deliveries`, `routine_delivery_cursor`, and `routine_context_receipts`;
the Telegram store owns `surface_identity`, `surface_sessions`, `conversations`,
`updates`, `delivery_messages`, and `surface_actions`, and binds the configured
agent in `surface_identity`; it refuses an earlier schema by name, so delete
the Telegram surface store after the Host cutover and re-pair. Because no transaction
spans them, a crash between steps leaves each store internally consistent and
every step repeatable; a surface whose stored agent id no longer exists fails
closed until it is provisioned against the current Host.

## Agent-driven changes

The full intended extension lifecycle and its staged proof plan are recorded in
[`renoa-extensions-north-star.md`](renoa-extensions-north-star.md).

The GUI is a surface, not the sole controller. `LocalHost` methods and each
agent's `plugin_manage` tool reach the same `PluginManager`; a future Waku
view will call that Host path rather than own extension state:

```text
human surface --\
                 -> effective session/agent policy -> Host operation
running agent --/                                      -> durable change
```

The current deliberate full-access policy permits search, lookup, inspect,
install, list, connect, and authorize without a second plugin approval prompt.
Service OAuth consent is authentication, not another Renoa permission decision.
A later restricted agent definition will gate the same management binding through its
one effective permission scope. An agent may exercise that authority but cannot
broaden it.
MCP registry attachments are visible at the next lookup; static runtime changes
wait for a new operation and never mutate an active manifest.

The kernel and the trusted Host enforcement path are outside agent-managed
modification.

### Persistent agents

The `renoa.agents` plugin exposes the canonical agent creation operation. The
primary schema accepts a name, standing instructions, exact machine tools,
provider/model/reasoning defaults, behavior, documents, and existing connections.
The preset id is optional. Caller values override template defaults; an explicit
empty tool selection removes every template machine grant. Fresh sessions consume
the stored model preference, while saved conversation selections remain separate.
Creation validates the requested model against the enabled provider catalog before
writing agent state. Receipt replay returns the original definition without
requiring another provider catalog lookup.

Identity, typed provenance, operational settings, exact machine selection, and
connections are persisted in one root row and normalized children. Each named
connection must have a complete catalog. Stable operation identities make identical
retries return the same child. Model tool-call identity determines the operation
id. Every child receives plugin management, discovery, and invocation by default.
An agent can select any machine grants for a child, but cannot change its own
grants through this plugin. Definition and document resolution happens again at
the next admitted turn; an active kernel operation retains its frozen manifest.

`agent_manage list` returns compact pages of 20 definitions with identity,
name, preset id, tools, and connections plus a continuation cursor. Exact
definition lookup is separate. Agent inventory includes persisted agents.
`agent_manage rename` edits the Host display name with an expected-current-name
check and a durable operation receipt; an agent may rename itself, and renaming
another agent requires the actor's enabled `renoa.agents` plugin.
Replay returns the original rename result; replaying a creation operation
returns the definition that operation committed, and later edits (rename,
capability changes) are not reflected. Agent identity, sessions, workspace,
tools, and connections remain intact.
Names should be short job labels, such as X Desk, News, or Research. Slack
projects display-name changes onto the existing channel by its retained ID.
Slack projects the Host agent inventory into dedicated private channels and
invites the operator. Its own catalog records provisioning state and conversation
bindings; creation recovery and external channel IDs remain surface concerns.
Plain messages in a dedicated channel route to that agent. The adapter
also persists a concise interface-capability snapshot with each admitted prompt,
so the model can distinguish automatic surface behavior from connected MCP
capabilities. This remains surface-owned input, not Slack policy in the Host or
kernel. Shared agent memory must not determine the active message surface. Slack also
supports optional `!agent <id>` routing in DMs and ordinary threads: admission persists the
target Agent alongside a fresh Session before acknowledging the command. Earlier
queued requests retain their targets. `!new` keeps the selected Agent; `!agent arcee` returns to the operator in a fresh conversation. A separate channel
thread can keep another conversation open. Agent working directories live
under the Host's `agents/<agent-id>/workspace` directory.

The following behavior describes the remaining product direction. Definition edits and structured generated-artifact management remain open. Host-owned
routines and Slack result delivery are implemented below.

For example, the user asks Arcee to create a news-digest agent with selected
sources, research tools, and a document-generation capability. Arcee uses Host
management operations to create the agent's definition and durable Agent
Instance, select available capabilities and account connections, and request a
Slack conversation binding. If a needed capability is missing, existing
plugin management supplies the installation/authentication path; mentioning
a tool in instructions never makes that tool available.

The agent is independently addressable and retains its own conversations,
working preferences, and output references. It can answer a direct message or
run a standing digest task on a schedule. It remains present when Arcee's
creating turn finishes, when either agent is idle, and across Host restarts.
A temporary delegated run may reuse agent execution primitives, but completing
that run and deleting a persistent agent are different lifecycle actions.

The Host must retain distinct relationships:

- the definition describes the agent's instructions and selected components;
- the Agent Instance identifies the persistent agent;
- the creating-agent relationship supports the operator/agent hierarchy
  without making the creator's current session own the agent's lifetime;
- a surface binding maps an external conversation to the intended agent and
  session; the Slack channel is not the Agent identity;
- a routine supplies a standing request, schedule, timezone, and delivery
  destination; each occurrence submits ordinary identifiable work; and
- an output reference identifies a durable artifact independently of its
  Slack attachment or notification.

The personal Renoa installation owns these records. A Slack application may
route several agents through separate channels or threads; creating a
agent does not require creating another Slack application or bot token.
The control panel may group agents by surface and nest agents beneath
their creator, while the underlying identities remain independent of that
presentation and can acquire another surface binding later.

Human controls and model-facing management tools must invoke the same typed
Host operations. Arcee can create and configure the agent; the agent
can update its own routine in response to the user's instruction. Changing
"daily at 16:00" to "daily at 14:00" updates the existing routine with an
explicit timezone and reports the next occurrence. It must not silently add a
second routine or mutate an already executing occurrence. A scheduled request
and an interactive request use the same session-admission and ordering rules.

Creation spans local durable records and external surface actions. Retrying an
interrupted creation must resolve the same agent and routine, reconcile
surface provisioning, and expose partial failure without claiming a usable
channel exists before its binding is confirmed. A routine update needs a
durable identity and revision check so concurrent edits cannot overwrite one
another unnoticed. These guarantees must be tested through actual Host
management callers rather than implemented only in an operator's prompt.

The first complete Slack milestone must prove: Arcee creates one news
agent; the user talks to it directly; a manual and a scheduled digest use
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
replacement, and manual-run operations. An agent manages its own routines; managing
or reading another agent's routines requires its enabled `renoa.agents` plugin.
Every agent discovers routine tools through `renoa.routines`. This is Host
management policy; selecting machine tools
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
agent, not a Slack channel. No surface identifiers or credentials appear in
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
Host APIs. An agent reads only its own results; reading another agent's results
requires the actor's stored selection to contain `agent_manage`.
Listing is bounded to 20 results, newest first, with sequence pagination; exact
lookup returns the retained task, output, and execution-session identity. This path
does not execute the routine and remains available from any surface.

Each routine has a stable execution session, separate from interactive chats, with
the same stored definition, workspace, and selected Host connections. Its standing
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
each agent's ready channel binding; one unbound agent does not block other agents.
The adapter marks posting intent before calling Slack; rate limits retry and uncertain
posts remain unknown rather than being blindly duplicated. Slack downtime delays
notification while the Host continues execution. Other surfaces can consume the same
Host result API with their own delivery cursors. This slice delivers text and durable
workspace file references; binary artifact upload and general workflow graphs remain
separate work. An agent's files remain retrievable through that agent's configured file tools.

The daemon launch JSON contains optional `home`, `model_bridge`, `providers`,
`provider`, `model`, `model_auth_store`, and optional `reasoning`, `mcp_adapter`,
`code_mode_worker`, `mcp_registry_adapter`, `shared_plugin_registry`,
`plugin_provider_families` (Host-reviewed family and exact-origin arrays), and `oauth_relay` (origin and private
device credential path). These are Host settings; there are no Slack tokens or
channel IDs. `deploy/renoa-host.service` runs this process independently of surfaces.
The supplied systemd unit loads `/etc/renoa/host.json` as `host-config` and the
shared relay device credential into its own credential directory. For this unit,
set the relay credential path to `/run/credentials/renoa-host.service/oauth-relay-device`;
it must not point into a surface service's credential mount.
Host schema 17 adds durable display-name edit receipts.
Schema 18 admits the one-time schedule variant; older readers cannot decode it.
Schema 19 adds routine deletion markers consumed by listing, lookup, and admission.
At that migration, all processes sharing the Host had to support schema 19 before
restarting. The current schema and later migrations are summarized in the
catalog schema history below. The integration tests exercise model-driven creation, agent
rescheduling, artifact generation, and recovery after losing the Host outcome receipt
without repeating the kernel's completed file operation.

## Catalog schema history

Host schemas 20 through 23 added the GitHub review tables
(`host_review_repositories`, `host_review_operations`, `host_review_requests`,
`host_review_deliveries`, `host_review_runs`, `host_review_jobs`, and
`host_review_publications`) and their execution timing.
Schema 24 adds `host_routine_owner_mutations` for authenticated owner receipts,
preserving existing agent receipts and their foreign-key restrictions.
Schema 27 is the clean break: it drops the retired agent-owned tables
(`host_agents` in its old shape, `host_bots`, `host_bot_tool_selections`,
`host_bot_tool_operations`, `host_bot_renames`, `profile_mcp_connections`,
`profile_mcp_tools`, `profile_skill_bindings`, `skill_source_rejections`, and
the agent skill bindings) and recreates the canonical `host_agents` root and its
normalized children in their current shape. Host identity, MCP catalogs,
connections, authorizations and credentials, installed plugins, skill revisions,
shared registry state, and provider credentials are preserved. The bounded reset
that clears agent rows and session directories, and the deployment procedure that
surrounds it, are described in the clean-break section above.
Schema 28 adds the immutable `result_json` snapshot to
`host_agent_creations`, so retrying a creation operation returns its exact
original result even after later edits to the live definition.
Schema 33 retires the GitHub review service. A schema 28–32 catalog drops the
seven `host_review_*` tables in place, each child before the parent it
references; the reset drops them from an earlier data root with the other
retired owners.

## Local CLI

The local CLI exposes these operations without a browser:

```text
renoa-host /absolute/host.json provision /absolute/provision.json
renoa-host /absolute/host.json agent-tools /absolute/edit.json
```

A provision file is the canonical creation request in camelCase JSON:
`operationId`, `name`, `instructions`, and optional `presetId`, `tools`, `model`, `behavior`, `documents`,
`connections`, and `routine`. `provision` calls the same durable creation
operation as `agent_manage` with a `System`/`Provisioning` actor and starts no
model or surface. Repeating the same request is idempotent;
reusing an operation id with a changed request conflicts. An `agent-tools` edit
contains `operation_id`, `id`, `expected_revision`, and `tools`.

## Locked decisions

1. The Host, not a surface or loop, resolves runtime composition.
2. `renoa-local` is the first concrete Host; no competing Host crate is added.
3. Agent identity and runtime assembly remain separate.
4. (Superseded by the canonical agent definition.) Agent definitions, installed
   capabilities, and resolved runtimes remain distinct.
5. The exact runtime is frozen by the kernel per operation.
6. GUI and agent changes will use the same Host management semantics.
7. (Superseded by exact capability selection.) V0 adds no permission model, but
   an agent runs with full access through exactly the capabilities its stored
   selection names.
8. Provider, workspace, surface, and future permission policy stay outside the
   kernel.
9. (Superseded by canonical agent storage.) The Host stores durable agent
   definitions in `host.sqlite3`, persists the exact agent id per session, and
   fails closed when that definition is unavailable.
10. (Superseded by agent-scoped selection.) Installed packages, MCP catalogs,
    and immutable skill revisions are one Host inventory; access and activation
    are explicitly agent-scoped.
11. (Superseded by agent-keyed traces.) Every trace database identifies its
    Agent and Session.
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
    remote execution, credential distribution, agent authorization, or
    surface state.

## Open decisions

- future Host schema migrations beyond the implemented catalog version;
- historical resolved-binding retention across explicit catalog/definition
  changes for unfinished-operation recovery;
- explicit skill deactivation, active-revision upgrade, source configuration,
  and immutable-package garbage collection;
- editing agent instructions and connection selections beyond the existing
  capability edit;
- permission vocabulary, scopes, and enforcement;
- public package discovery, updates, rollback, removal, and garbage collection;
- Host management beyond the personal HTTPS panel, including remote CLI
  enrollment and broader configuration operations;
- whether capability changes pause and continue a task through one or more
  internal operations; and
- process placement and supervision for multiple concurrent local Agent Instances
  beyond the existing durable agent definitions and multiple Sessions per Agent;
- credential, connection, and agent-definition distribution
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
3. the frozen manifest names the model and the exact workspace tool bindings;
4. the existing real workspace edit and Bash cancellation paths remain green;
   and
5. ACP, RCP, package installation, permissions, and UI code remained outside
   that coherent foundation slice.

The next consumer slice is also complete: ACP talks only to `LocalHost` and
`AgentSession`, loads and binds provisioned Alpha agent identities, admits stable turn IDs,
streams transient model and tool events, durably cancels active effects, and
projects final answers from semantic history. Exact redelivery is proven both
within one process and after restart. Concurrent admission cannot leave ghost
work, pre-cancelled turns are not admitted, unknown effects do not wedge the
session, project instructions refresh per turn, session publication is atomic,
and torn runtime logs remain appendable. Per-turn trace rows preserve ordered
model/tool flow without entering kernel truth. The legacy harness crate is
retired.

The Host is now agent-generic while ACP deliberately selects one configured
Alpha agent. A deterministic non-Alpha agent definition reaches the model with
its own instructions,
persists its exact Agent/Session trace identity, survives Host restart,
and fails closed when reopened without its stored definition. One MCP
catalog can be selected by two agents without copying it, while selecting it
for one agent alone does not leak access to the other. This prepares the Host
for additional agent definitions without inventing surface or permission policy.

The first hosted surface registers Arcee and maps each allowlisted private
Telegram topic to one caller-identified Host session. It persists an update
before advancing the polling offset, preserves request identity across process
loss, re-drives kernel-owned execution, and never blindly repeats an uncertain
Telegram final send. `/new`, `/compact`, `/status`, `/model`, `/reasoning`,
`/cancel`, native draft
stopping, bounded live drafts, and exact-agent execution cross the real Host
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
attachments, and restores them after process restart. Every assembled agent
runtime is offered two fixed registry tools regardless of catalog size, and
the stored selection decides whether they bind. Search is a bounded
`SafeToReplay` read; execute carries an exact catalog reference through the
normal loop and kernel as a `NeverReplay` effect. Exact registration retries
converge, identity changes conflict, failed refresh publication preserves the
previous snapshot, stale references fail closed, structured details stay
outside model context, unknown calls are not replayed, and schemas v1 and v2
migrate to v3 without losing catalog state. A live registry object observes a
newly committed attachment, and broad browsing of 1,000 tools exposes no schema. No
kernel type or table changed.

The first OAuth connection path is also complete. One `plugin_manage`
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
content digest. The public `LocalHost::manage_plugin` API and fixed
`plugin_manage` tool consume
one canonical typed request dispatcher. Complete and model-facing schemas are
derived from that contract. Local packages, standalone skills, pinned public
GitHub sources, researched MCP endpoints, and installed revisions converge on
one immutable library. Host bootstrap MCP registration also creates a package.
Schema v6 stores package metadata, public MCP
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
`plugin_search` with source `official_mcp_registry` queries the official MCP Registry's stable `v0.1`
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
kernel, ACP, Waku, or RCP type changed. List uses typed, bounded revision-bound
cursor pages of at most 200 facts and rejects a stale cursor if that Host
inventory changes.

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
rejected. No credential, connection, agent selection, session record,
kernel type, RCP type, or surface contract is copied.
This synchronization path changes the frozen `plugin_manage` implementation
from revision 9 to revision 10. An unfinished revision-9 operation fails closed
after upgrade instead of acquiring network synchronization under its old
manifest.

Schema 31 owns agent plugin activation, historical revision identities, and
durable activation receipts. Multi-server packages use Host-reviewed provider
families before admission; portable metadata cannot authorize itself. Legacy
plugin selections require the explicit Host reset; unselected library revisions
migrate with verified MCP ownership and require explicit current admission
through install or activate before their retained accounts can be selected.

The canonical plugin API binding is `renoa-plugin-api-v3`; source-contract
changes cannot replay under an older frozen management manifest. The full
source and lifecycle contract is in `renoa-extensions-north-star.md`.
