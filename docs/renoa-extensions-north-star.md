# Renoa extension system north star

## Status and authority

This document defines the intended product and architecture direction for
installing, connecting, selecting, and using replaceable Renoa capabilities.
It is the north star for extension work and the canonical contract for the
implemented local Agent Plugins path. It is not a public package-registry,
general permission, or cross-node distribution contract.

[`renoa-kernel-v0.md`](renoa-kernel-v0.md) remains authoritative for durable
local execution. [`renoa-host-v0.md`](renoa-host-v0.md) remains authoritative
for runtime composition. [`rcp-v0.md`](rcp-v0.md) remains authoritative for
cross-surface and cross-node task continuity. This document must preserve those
boundaries.

The decisions explicitly marked as locked below guide implementation. The open
decisions must remain open until a real consumer and test prove their shape.
No placeholder type, table, field, trait, or transport should make an open
decision appear settled.

## North star

> A standards-compatible skill or MCP integration should normally become
> usable by adding and reviewing one package and, when needed, one connection,
> not by editing the Renoa kernel, agent loop, model adapter, or surface.

When an agent discovers that it lacks a capability, the intended experience is:

```text
agent or human identifies a missing capability
  -> Host searches a replaceable discovery source or inspects an exact source
  -> the current agent/session policy permits or rejects the requested change
  -> Host installs the package and establishes any required connection
  -> selected components become available to selected agent profiles
  -> the next safe capability lookup or operation uses the committed change
```

The agent and GUI use the same Host semantics. Capability management is covered
by the agent/session's one effective permission scope; it does not invent a
second plugin-specific approval system. Alpha v1 currently has deliberate full
access, so it may perform these changes directly. A credential prompt or OAuth
consent is service authentication, not another Renoa permission decision.

## Product outcome

The finished system should make all of these ordinary:

- install one portable package containing a skill, one or more MCP servers, or
  both;
- connect more than one account to the same integration, such as personal and
  work Google accounts;
- expose selected tools or skills to Alpha, a GitHub review agent, or another
  profile without duplicating the underlying installation;
- add a typical company-hosted remote MCP integration through package data
  alone when Renoa already supports its transport and authentication method;
- install directly from an exact local directory or Git source without first
  publishing to a Renoa marketplace;
- update or roll back a package without changing an active operation;
- inspect what code, commands, network endpoints, schemas, and requested
  credentials will be involved before installation;
- see useful typed failures instead of silent omission, generic internal
  errors, or a permanently spinning surface; and
- request the same logical capability from different surfaces while one Host
  remains the authority that assembles the agent.

Adding a new transport, authentication mechanism, component type, or execution
primitive may require adapter work once. Adding another package that uses an
already supported mechanism should not.

## Ownership

### Kernel

The non-replaceable kernel continues to own only durable execution truth:
command admission, operation ordering, frozen runtime identity, exact effect
intent, settlement or uncertainty, cancellation, checkpoints, semantic events,
and recovery.

The kernel does not parse plugin manifests, discover MCP tools, authenticate
accounts, choose profile capabilities, search a marketplace, or interpret
skills. It receives ordinary frozen loop and effect bindings resolved by the
Host.

### Host

The Host is the trusted extension control plane and assembly layer. The first
implementation remains `renoa-local`; no competing Host crate is introduced.
It owns:

- the installed capability library;
- package inspection, installation, update, rollback, and removal;
- integration definitions and account connections;
- secret references and authentication coordination;
- discovered tool catalogs and schema revisions;
- profile selection and future authorization policy;
- conversion of selected components into native Renoa runtime bindings; and
- the typed management semantics shared by human surfaces and agent tools.

### Component adapters

Each component keeps its native execution contract:

- a skill supplies versioned instructions and supporting files;
- an MCP server initially supplies tools through MCP;
- a native Renoa tool implements `renoa-agent::Tool`;
- a model provider implements the model boundary;
- a context or compaction strategy implements its own loop-facing boundary;
- a future loop plugin implements the kernel's decision-only loop contract.

There is no universal runtime `Plugin` trait. The universal part is the
installable package envelope and Host lifecycle. Execution remains typed by
component responsibility.

### Surfaces

Waku, a future mobile application, a CLI, or another surface may present the
library, plans, connections, and profile selections. A surface does not own
those records and does not build a parallel runtime. ACP remains the current
agent-facing chat boundary; the Host management transport will be selected
only with its first real surface consumer.

### RCP

RCP remains task continuity. It does not become a plugin protocol, marketplace,
secret synchronizer, or MCP transport. A future node may advertise that it can
resolve a required capability, and multiple nodes may install the same package
digest, but execution placement and credential availability remain separate
facts.

### Public package catalog

A separate public `renoa-plugins` repository will contain reviewed package
source directories. It is a discovery and distribution source, never execution
truth or implicit authorization. The Host must also accept exact packages from
other sources.

No Renoa-specific marketplace index is defined before a browsing or update
consumer needs one. Early installs may identify an exact repository, path, and
commit directly.

The first implemented discovery source is the public integrations.sh REST API
behind `adapters/integration-catalog-node`. It is a replaceable hint source,
not a package registry or runtime dependency. Search returns bounded normalized
MCP candidates. Add refetches the selected record and verifies its exact
content reference before generating a standard Agent Plugin package. A
missing, stale, malformed, or unavailable record produces explicit guidance to
use official web research or an exact local Agent Plugin package; Renoa never
guesses an endpoint.

The implemented `add` operation has three source forms: one exact catalog
reference, one MCP definition backed by an official HTTPS documentation URL,
or one local Agent Plugins directory bound to the digest returned by `inspect`.
All three converge on the same immutable package store and component loaders.
The digest requirement prevents a crash replay from observing different bytes
at the same mutable path. Package installation and skill loading happen before
any MCP connection attempt. A missing credential, authorization failure,
unsupported server choice, or unreachable endpoint therefore leaves the
package installed and returns that exact partial state; it never makes the
package disappear or fabricates a successful connection.

## Vocabulary

### Plugin package

One portable directory with a root `plugin.json` and optional standard
components. Renoa uses Agent Plugins 1.0 as the portable package floor.

The common authoring shape is deliberately small:

```text
plugin.json
skills/       optional skill components
mcp.json      optional MCP server components
```

This is a package envelope, not a promise that every component executes through
one generic plugin interface.

### Component

One independently loadable item supplied by a package. Agent Plugins 1.0
standardizes two component types: skills and MCP server entries. Renoa may add
client-owned behavior only through the standard reverse-domain extension
mechanism, without pretending that behavior is portable.

### Skill

An Agent Skills-compatible `SKILL.md` directory containing model instructions
and optional scripts, references, or assets. A skill is not a callable tool
merely because it may teach an agent how to use tools.

### Integration

A service or capability definition such as Google Drive, GitHub, or one MCP
endpoint. A direct MCP definition may exist without a surrounding package.

### Connection

One configured and, when required, authenticated instance of an integration,
such as `google.personal` or `google.work`. A package is installed once; it may
have zero, one, or many connections.

### Tool catalog entry

One exact tool name, description, input schema, server identity, and discovered
catalog revision associated with a connection. Discovery does not by itself
make the tool model-visible.

### Profile binding

A declarative selection that asks the Host to make a component available to an
agent profile. It is not yet the vocabulary for a complete permission system.
Future authorization may further restrict the effective result.

### Resolved runtime binding

The exact native skill, tool, adapter, schema, implementation revision, and
recovery declaration selected for one operation. The kernel freezes this, not
the mutable package or connection record.

### Registry or marketplace

A discovery index over package sources. It does not install, connect, enable,
authorize, or execute anything by itself.

The product may use the informal word "connector" for an integration plus its
connection experience. Core code and contracts should use the more precise
terms above.

## Distinct lifecycle states

These states must never collapse into one boolean such as `enabled`:

```text
source discovered
  -> source inspected
  -> effective agent/session policy permits the change
  -> package installed immutably
  -> integration definition registered
  -> connection configured/authenticated
  -> component catalog discovered
  -> profile binding selected
  -> component becomes resolvable through a fixed registry or next runtime
  -> runtime frozen by the kernel
```

Installation does not imply a connection. A connection does not imply profile
selection. Profile selection does not bypass authorization. Catalog discovery
does not make every schema part of model context. Mutable Host state never
changes the frozen implementation of an admitted operation. A fixed registry
tool may read a later committed snapshot, but an exact referenced invocation
must reject schema drift rather than substitute it.

## End-to-end management flow

### Inspect

The Host resolves an exact local or remote source without executing package
code. It validates the package boundary and manifest version, records source
and license evidence, and reports the manifest, supported MCP entries, and
package-level notices it understands. Skill components are validated from the
immutable installed tree before they are bound. Installing a package never
grants permission to execute its scripts or start its servers.

### Content-bound request

Inspection returns the exact package digest, portable metadata, supported MCP
entries, public endpoints and headers, and isolated component notices. The
digest is the install precondition: changed source requires another inspection.
Renoa does not create a second durable approval object in this slice.

### Authorize and install

The effective agent/session permission policy authorizes or rejects the
management tool itself. Installation copies the inspected contents through a
staging location into an immutable content-addressed store, then durably
publishes the installed record before acknowledging success. A changed source
requires a new digest. V0 full access permits this operation.

### Connect

The Host establishes one connection using a supported authentication adapter.
Package data may declare public configuration but never carries secret values.
Durable Host state stores only a reference to secret material held by a
dedicated credential boundary.

### Discover

The Host obtains and validates the component catalog. MCP tool descriptions,
schemas, negotiated protocol behavior, and cache identity are preserved, and
entries are normalized into a deterministic order. A refresh publishes one
complete new catalog or leaves the prior complete catalog intact; partial
refreshes never become active.

### Select and resolve

A profile chooses components or connections by stable identity. The Host
applies current scope and future policy. Small static capabilities become
ordinary resolved bindings for the next operation. Large MCP catalogs attach
to a fixed search/load/execute registry; only an exact loaded schema enters
history, and execution resolves its catalog-bound reference. These are two
composition strategies, not two execution paths through the kernel.

### Execute

The kernel freezes exact component and adapter revisions through the existing
runtime manifest and configuration digest. External MCP calls pass through the
same intent-before-effect boundary as native tools.

### Update, disable, and remove

An update installs another immutable digest; it does not rewrite the prior
package in place. Static runtime changes apply at the next operation. A
registry attachment is visible at the next safe registry read without restart.
Removal cannot delete content still required to recover an unfinished
operation. Rollback is selection of a previously installed compatible digest,
not restoration from mutable leftovers.

## Durable and security invariants

1. The Host persists an admitted management change before acknowledging it.
2. Retried management commands use stable identities and cannot create
   duplicate installations, connections, or bindings.
3. Installed package contents are immutable and addressed by a verified digest.
4. Source revision and license are recorded before Renoa adapts or ships
   upstream material.
5. A package source, registry entry, manifest, MCP server, and tool schema are
   untrusted boundary data and are parsed and bounded before entering typed
   Host state.
6. Package paths remain inside the filesystem-resolved package root. Archive
   extraction, symlinks, redirects, commands, arguments, and URLs are validated
   at their boundaries.
7. Inspection and installation never execute package code. Starting a server
   or invoking a script requires a separately resolved runtime capability.
8. Secret values never enter a package, package catalog, model prompt, tool
   schema, trace payload, or public registry record.
9. Only profile-selected skills and model-relevant instructions enter the
   system prompt. Only effective tool specifications enter the model API tool
   field.
10. Manifests, source metadata, auth bookkeeping, runtime identities, recovery
   declarations, and unused tool schemas do not pollute model context.
11. An active operation keeps its exact frozen runtime. Fixed registry reads may
    see later committed Host state, but every remote invocation carries an
    immutable catalog reference and stale references fail before dispatch.
12. Discovery and other proven read-only operations may retry under a bounded,
    cancellable policy. A possibly dispatched external tool call does not retry
    unless its adapter has a proven idempotence contract.
13. MCP tools default to `NeverReplay`. A timeout, cancellation, transport loss,
    or process loss after dispatch that cannot prove the remote result becomes
    outcome unknown rather than a fabricated failure or automatic repeat.
14. Cancellation is not reported complete until adapter-owned work has stopped
    or the adapter has honestly reported that the remote outcome is unknown.
15. Unsupported result content, protocol behavior, schema drift, and auth
    requirements fail visibly at the narrowest component boundary. No data is
    silently dropped to keep a plugin appearing healthy.
16. A registry can suggest a package but cannot approve it, authenticate it,
    bind it to a profile, or make it authoritative.
17. No component installer or adapter may write kernel storage directly.

Renoa does not claim exactly-once behavior for an external service. A stable
kernel `EffectId` may be forwarded as an idempotency hint when a reviewed
service supports it, but the Host must not invent a guarantee the service does
not provide.

## Model-context discipline

Extension breadth must not become context pollution. For one resolved
operation, the model receives only:

1. the profile's instructions, two fixed skill-registry definitions, and only
   deliberately activated full skill content;
2. the durable context projection; and
3. the six local tools, three fixed MCP-registry tools, and one fixed
   `extension_manage` definition required for the current full-access profile.

Package manifests, marketplace descriptions, setup instructions, connection
state, OAuth scopes, environment variables, secret references, process
bookkeeping, schema hashes, and runtime manifests remain outside model context.
Tool failures that help the model recover are returned as bounded model-visible
tool results; operational diagnostics and sensitive details remain in Host
trace data. The current manager fails rather than truncates an encoded result
that exceeds the local 50 KiB tool-output boundary.

Alpha's first skill path searches compact name/description metadata through
`skill_search` and activates one selected name through `skill_load`. Search
returns at most 200 matches and nothing per match beyond name and short
description. A workspace skill explicitly overrides a same-named global skill,
and a global skill overrides a package-provided skill. A newer revision of the
same plugin replaces that plugin's bindings. Two different plugins with the
same skill name do not get an arbitrary digest-based winner: the first binding
remains available and the other plugin receives a visible component rejection.
Neither the complete catalog nor any skill body is injected up front. A load
resolves and persists one exact revision internally before returning its
complete instructions. Later operations reattach that revision as standing
instructions, including after restart or compaction, while the historical load
result is projected to a short receipt for the model. The durable journal is
never rewritten. The Host records the activating command so retrying an
unfinished command reconstructs its original frozen binding instead of
silently gaining the new revision. One session cannot activate two revisions
of the same skill name.

## Physical ownership

The intended physical separation is:

```text
Renoa repository
  crates/renoa-local/          Host library, assembly, and management semantics
  adapters/mcp-client-node/    replaceable MCP protocol implementation
  adapters/integration-catalog-node/
                               replaceable discovery-only REST adapter

renoa-plugins repository
  official/<plugin>/           Renoa-owned packages
  third_party/<plugin>/        packages for external services

Renoa data directory
  skills/<digest>/             immutable imported Agent Skill revisions
  plugins/<digest>/            immutable Agent Plugin packages
  Host catalog                 installations, integrations, connections,
                               skill revisions, source/profile bindings,
                               session activations, and other components
  sessions/<session>/          existing kernel and trace truth

local credential sources
  secret material owned by a platform store or authenticated CLI
```

Schema v6 adds installed Agent Plugin metadata, supported MCP entries, public
request headers, and named Secret Service bearer references. Schema v7
preserves standard plugin homepage provenance and adds the package-skill scope
without changing existing global or workspace bindings. Schema v8 adds OAuth
connection references, non-secret recovery phases, and bounded terminal
receipts keyed by stable session/command/tool-call identity. OAuth client state,
tokens, and remote failure text never enter `host.sqlite3`; all secret values
are resolved just in time from Secret Service. Cross-platform secret stores,
cross-node credential placement, permission, package-registry, update, and
removal designs remain open.

`third_party` means that the service is external to Renoa. It does not assert
that the named company authored, reviewed, or endorsed the package. Provenance
and authorship must be represented accurately.

## Portability across profiles, surfaces, and nodes

One installed package and connection may supply components to multiple profiles
on the same Host. Every surface controlling that Host sees the same durable
library because no surface owns a private copy.

Across nodes, package identity is portable by source revision and content
digest. Connections are node-local until a separate credential and placement
design proves otherwise. A laptop and VPS may install the same package while
holding different accounts, filesystem access, or no usable connection at all.
Raw secrets are never synchronized merely because package identity is shared.

RCP may eventually route a task to a node that can resolve its required
runtime, but RCP remains independent of package loading and does not carry
model prompts, tool schemas, or credentials as continuity data.

## Standards and implementation evidence

Reviewed on 2026-08-29. Renoa copied no upstream implementation source for the
skill or Agent Plugin paths. The YAML and URL parsers are pinned dependencies.

| Source | Exact revision | License evidence | Renoa use |
| --- | --- | --- | --- |
| [Agent Plugins 1.0](https://github.com/agentplugins/agent-plugins-spec/tree/ff8ab5e392cc87bd88d87c060815a87490e51003) | `ff8ab5e392cc87bd88d87c060815a87490e51003` | Specification and docs CC-BY-4.0; schemas and software Apache-2.0 | Portable package, skill, MCP, containment, discovery, and client-extension floor |
| [Agent Skills](https://github.com/agentskills/agentskills/tree/69ef37e9424c0a7ea9dd2293b559e43ec8176379) | `69ef37e9424c0a7ea9dd2293b559e43ec8176379` | Code Apache-2.0; documentation CC-BY-4.0 | Portable `SKILL.md` structure and progressive-disclosure model |
| [OpenCode v2](https://github.com/anomalyco/opencode/tree/f1521000ece5fdd9f372dcfbd126d3d89642f3ce) | `f1521000ece5fdd9f372dcfbd126d3d89642f3ce` | MIT | One narrow skill tool, metadata-first discovery, full body/base directory on demand, filesystem refresh, and durable activation evidence; Renoa does not adopt silent last-writer-wins identity |
| [Claude Code skill lifecycle](https://code.claude.com/docs/en/slash-commands) | official documentation reviewed 2026-08-27 | Anthropic documentation terms | Evidence that invoked skill instructions must be deliberately carried across compaction; Renoa fails at its exact bound instead of truncating or dropping an active revision |
| [Codex core skill loader](https://github.com/openai/codex/blob/main/codex-rs/core-skills/src/loader.rs) | `main` reviewed 2026-08-27 | Apache-2.0 repository | Bounded discovery, standard `.agents/skills` support, and explicit refresh behavior |
| [serde-saphyr 1.1.0](https://github.com/bourumir-wyngs/serde-saphyr/tree/ad5c614bd437f9c3dbf65b158de24cb3a07cda9d) | tag commit `ad5c614bd437f9c3dbf65b158de24cb3a07cda9d` | MIT OR Apache-2.0 | Deserialize-only YAML frontmatter dependency with default features disabled; no source adapted |
| [MCP 2026-07-28](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/5f5440bb26a62e2cf3440b92da5a667efa03b267) | tag commit `5f5440bb26a62e2cf3440b92da5a667efa03b267` | Repository records an Apache-2.0 transition with remaining MIT material and CC-BY-4.0 documentation | Current remote tool protocol semantics |
| [MCP TypeScript client 2.0.0](https://github.com/modelcontextprotocol/typescript-sdk/tree/cc4b41617ce3601b1290d67216ea0b194a3cd9ac) | tag commit `cc4b41617ce3601b1290d67216ea0b194a3cd9ac` | Published package declares MIT; source repository records the broader MCP license transition | Maintained implementation behind Renoa's narrow process adapter |
| [GitHub MCP server](https://github.com/github/github-mcp-server/tree/a00dc319edcb5f8a10f118b1dad649c94928aac4) | `a00dc319edcb5f8a10f118b1dad649c94928aac4` | MIT | First real read-only remote connection; no upstream server source copied |
| [Exa MCP server](https://github.com/exa-labs/exa-mcp-server/tree/15ffb50519e719dc791cdc750ce5ed1934c0a1ed) | `15ffb50519e719dc791cdc750ce5ed1934c0a1ed` | MIT | First real Agent Plugins/API-key package shape; endpoint and public source header consumed through the generic loader, with no server source copied |
| [Cursor plugins](https://github.com/cursor/plugins/tree/bdf7aa355337897f167153e05069aca505dae17c) | `bdf7aa355337897f167153e05069aca505dae17c` | MIT at reviewed revision | One-directory-per-package marketplace organization; Cursor-specific manifests are not Renoa's portable contract |
| [Executor](https://github.com/UsefulSoftwareCo/executor/tree/7c12aeea390225291ce4c97865b392237ee7934d) | `7c12aeea390225291ce4c97865b392237ee7934d` | MIT | Evidence for separating integrations, authenticated connections, discovered tools, and just-in-time secret resolution |
| [integrations.sh](https://github.com/UsefulSoftwareCo/integrations/tree/5219a70601c8c356146aa1bc7429e571cf64fbf1) | `5219a70601c8c356146aa1bc7429e571cf64fbf1` | MIT | Replaceable ground-zero MCP metadata discovery through the public REST API; no runtime source copied and no catalog record becomes authoritative without refetch, validation, MCP discovery, and Host publication |

Agent Plugins 1.0 standardizes only skills and MCP server entries. It explicitly
leaves registries, installation, permissions, trust, caching, and product UX to
the client. Renoa owns those responsibilities in the Host rather than extending
the portable manifest with accidental product policy.

MCP 2026-07-28 removes protocol-level sessions and makes each request
self-describing. The official TypeScript SDK v2 supports the revision, but
modern version negotiation is an explicit client choice. Renoa prefers the
modern revision and uses the SDK's reviewed legacy negotiation only when the
modern probe proves that fallback is required. The exact negotiated revision
is stored with the complete catalog and reused for calls; a call never
renegotiates to a different revision or retries `tools/call`. The same pinned
client owns OAuth discovery, PKCE, client registration, code exchange, and
refresh behind Renoa's bounded adapter process; the Host remains authoritative
for durable phase, endpoint binding, secret placement, and replay decisions.

## Definition of streamlined

The extension path is streamlined when these statements are true:

1. A skill-only package requires no Renoa source-code change.
2. A remote MCP package using an already supported transport and auth method
   requires no Renoa source-code change.
3. A package can be inspected and installed from a local directory or exact Git
   source without marketplace publication.
4. One package may define several integrations, and one integration may have
   several connections, without copying the package.
5. One discovered component may be selected by several profiles without
   duplicating its implementation or credentials.
6. Static component changes affect future operation manifests; registry
   attachments become visible at the next bounded lookup without restart.
7. A package author uses the Agent Plugins schemas rather than a Renoa-only
   layout for portable skills and MCP servers.
8. Invalid, incompatible, or unsupported components fail independently where
   the standard permits, with exact diagnostics.
9. The user can understand source, code, endpoints, credential references,
   selected profiles, and expected changes before installation.
10. An agent whose effective permission scope allows capability management can
    drive the same Host flow as the GUI without modifying Host or kernel files.

## Implementation sequence and proof gates

Each slice must leave the repository coherent and pass its proof gate before
the next begins.

Slices 1 through 8 are complete. Slice 9 has its local Host and agent-tool path;
the first surface consumer and general permission policy remain open.

### 1. Integration contract

Create a versioned contract for one direct, unauthenticated Streamable HTTP MCP
integration. Lock only the state, failure, cancellation, refresh, naming, and
recovery semantics consumed by that path.

The resulting contract is [`renoa-mcp-v0.md`](renoa-mcp-v0.md).

Proof gate: the contract names every owner, state transition, retry boundary,
and non-goal without adding production fields or dependencies.

### 2. MCP process adapter

Build a narrow Node adapter using the pinned official MCP TypeScript client.
Support current Streamable HTTP, deterministic discovery, one tool call,
bounded ordered results, cancellation, process supervision, and typed terminal
errors. The initial proof is unauthenticated; the later Host-management slice
adds OAuth without adding a second adapter. Do not add stdio, resources,
prompts, apps, or tasks.

Proof gate: deterministic local-server tests cover negotiation, discovery,
invocation, malformed responses, timeouts before and after dispatch,
cancellation, output limits, process exit, and secret-safe diagnostics.

### 3. Durable Host catalog

Persist one integration, one no-auth connection, one complete discovered tool
catalog, and one Alpha profile binding. Publish related changes transactionally
and recover them after restart. Do not create a generic storage trait.

Proof gate: exact retries converge, changed identities conflict, partial writes
never become active, and catalog replacement is atomic.

### 4. Alpha and kernel vertical proof

Resolve one MCP tool into the existing `renoa-agent::Tool` and
`AgentToolBinding` path. Use a deterministic model and local MCP server to run
one complete Alpha operation through the real kernel.

Proof gate: only an explicitly resolved schema reaches the model, exact
references are persisted, restart does not duplicate a settled call, a
possibly dispatched call is never replayed, schema drift never substitutes a
new invocation, and useful tool errors return to the model.

### 5. Authenticated GitHub connection

Add one real credential-backed connection without building a general secret
store. The Host stores only an exact `github.com`/account reference, resolves
the bearer token just in time through authenticated `gh`, and scopes it to
GitHub's read-only remote MCP endpoint.

Proof gate: the token exists only at the credential/adapter boundary, is
redacted from every returned value and diagnostic, never enters Host SQLite or
model context, and the real endpoint catalog refresh succeeds.

### 6. Deferred registry and hot loading

Replace per-tool model advertisement with three fixed Host tools: bounded
search without schemas, exact schema loading, and exact-reference execution.
Attach whole connections to profiles, migrate existing per-tool selections,
and read committed Host state on each registry call.

Proof gate: 1,000 catalog entries still produce only three model API schemas;
load returns only requested exact schemas; stale catalog references fail before
dispatch; a live registry object sees a newly committed attachment; and the
real MCP result, error, uncertainty, restart, and secret boundaries remain
unchanged.

### 7. Host-owned Agent Skills path

Import standard global `~/.agents/skills` and workspace `.agents/skills`
directories into immutable Host-owned revisions. Expose two fixed model tools:
bounded metadata search and exact activation. Rescan sources on every search so
a live Alpha session can discover a new skill without restart. Pin activated
revisions to the session and reattach their exact instructions across later
operations, restart, and compaction.

Proof gate: global and workspace collisions stay exact with workspace priority;
invalid entries are isolated; failed source scans preserve the prior complete
snapshot; search returns up to 200 entries with exactly name and description;
hot additions are visible to an existing session; source edits cannot silently
replace active content; duplicate names cannot activate two revisions; bounded
context fails instead of truncating; full durable results remain intact while
later model context uses receipts; and the real Alpha path survives compaction
and Host restart with the constant tool set.

### 8. Agent Plugins local loader

Load and validate one local Agent Plugins 1.0 directory using locally pinned
schemas. Enforce package containment and component-level failure boundaries,
then publish an immutable content-addressed installation. Map its skills onto
the existing Agent Skills registry and its MCP entries onto the already proven
integration path. Create the separate `renoa-plugins` repository only when the
first reviewed package is ready.

Proof gate: malformed manifests, unsupported versions, denied symlinks, changed
contents, immutable publication, and independent component failures are
deterministic and leave no partially installed durable record. An Exa-shaped
package reaches the real MCP adapter with its public header and a just-in-time
Secret Service bearer without persisting the key. Valid package skills hot-load
without restart, invalid skills are isolated, and cross-plugin name collisions
are reported instead of resolved by storage order.

### 9. Shared Host management

Define typed Host management operations consumed by both a surface and one
fixed agent tool. The effective agent/session policy governs those operations;
there is no nested plugin approval path. The resulting capability becomes
available only at the next safe registry lookup or operation boundary,
according to its resolved component type.

Current proof: `LocalHost` and `extension_manage` call the same manager;
inspect/install retries are content-bound and idempotent; local package adds
require the inspected digest; connect reuses the
existing catalog/attachment path; search and add use a replaceable
integrations.sh REST adapter; catalog, officially researched MCP, and local
package sources all normalize into one immutable installation path; standard
package skills enter the existing skill registry; and connection discovery
runs only after installation. Connection failure reports the retained package,
skill result, package notices, exact safe service error, and any known
connection identity. One live `skill_search` or `tool_search` registry observes
the committed component without restart. Retrying the same source converges on
the same digest and connection identity. OAuth connections use that same path:
the Host owns exact loopback browser authorization and refresh, Secret Service
owns credential values, SQLite owns only the connection reference and durable
phase plus semantic terminal receipts, and uncertain exchanges are never
replayed.
Remaining proof: wire the first GUI consumer and resolve the general permission
vocabulary without letting an agent broaden its own effective scope.

### 10. Later fabric work

Only after the local path is mature should Renoa design node capability
advertisement, package availability, or placement-aware resolution. That work
must preserve `rcp-v0.md` and must not synchronize secrets or package execution
through the task journal.

## Locked decisions

1. The kernel is not replaceable and is not a package manager.
2. The Host owns extension lifecycle, connections, profile selection, and
   runtime assembly.
3. `renoa-local` remains the first Host; extension work does not create another
   Host framework.
4. Agent Plugins 1.0 is Renoa's portable package floor for skills and MCP
   servers.
5. Portable packaging does not imply one universal component execution trait.
6. MCP is a replaceable tool adapter behind the Host, not a kernel or RCP
   protocol.
7. Installation, connection, catalog discovery, profile selection,
   authorization, and runtime resolution remain distinct states.
8. Installed package contents are immutable and content-addressed.
9. The public package catalog is separate from the Renoa core repository and is
   never trusted merely because it is listed.
10. Direct exact-source installation works without a marketplace.
11. Human surfaces and authorized agents ultimately use the same typed Host
    management semantics.
12. Capability management is governed by the agent/session's effective
    permission scope; it never expands that scope.
13. Capability changes never mutate the active runtime. Static bindings change
    at a future operation; fixed registry tools may read later committed state
    and must reject stale exact references.
14. Only effective instructions and tool definitions enter model context.
15. Secrets remain behind a dedicated credential boundary and are referenced,
    not copied, by Host records.
16. Possibly dispatched external calls are not automatically replayed without
    a proven idempotence contract.
17. RCP remains continuity and placement infrastructure, not extension
    execution or credential distribution.
18. Every slice is proved through the real boundary it introduces before the
    next layer is built.
19. Skills are instructions and files, never an implicit tool or permission
    grant. The experimental Agent Skills `allowed-tools` field is rejected until
    Renoa has a real permission consumer.
20. Skill search returns at most 200 matches containing only name and short
    description. Full content and exact revision identity enter context only
    after a load by name.
21. Active skill revisions are Host-owned and session-pinned. Source edits are
    hot-discoverable but cannot silently replace an active revision.
22. Package-provided skills reuse the Agent Skills registry. Precedence is
    workspace, then global, then plugin; different plugins never resolve a
    same-named skill by arbitrary digest order.
23. OAuth remains a Host-owned connection/authentication flow. The replay-safe
    management binding may carry it only with the stable
    session/command/tool-call identity, resumable callback state,
    endpoint-bound Secret Service bundle, terminal receipt, and process-death
    recovery path now consumed by implementation and tests.

## Open decisions

- future Host schema migrations beyond v8 and storage for permissions;
- model-visible naming and collision handling for tools from many connections;
- exact catalog freshness, cache-hint, and refresh policy;
- cross-platform secret-store selection, account recovery, revocation, and
  secret sync;
- headless/device OAuth flows and non-Linux browser launchers;
- permission vocabulary, scopes, policy inheritance, and enforcement;
- profile persistence, inheritance, and Agent Instance overrides;
- Host management transport for surfaces;
- registry index, search, signing, trust, update, and review policy;
- package garbage collection and unfinished-runtime retention policy;
- OAuth terminal-receipt garbage collection after a proven Host/kernel
  settlement boundary;
- historical resolved-binding retention after catalog or profile changes;
- when to add stdio MCP servers and how to constrain their process and
  filesystem authority;
- ancestor-directory skill discovery, explicit profile configuration, manual
  revision upgrade, deactivation, and garbage collection;
- support for MCP resources, prompts, apps, tasks, or future protocol
  extensions;
- compatibility policy beyond the SDK-supported legacy MCP revisions;
- process lifetime and multiplexing for the MCP adapter;
- node capability advertisement and cross-node Host configuration; and
- stronger service-specific idempotency, reconciliation, or callback contracts.

The first vertical proof is complete. These remain boundaries against guessing
beyond the next real consumer.

## Remaining v0 non-goals

The first direct MCP slice does not implement:

- a general package registry, marketplace, remote package download, or archive
  extraction inside the manager;
- GUI credential entry, headless OAuth, or general secret synchronization;
- stdio MCP servers;
- MCP resources, prompts, apps, tasks, sampling, or elicitation;
- Waku extension settings or management UI;
- a general permission system;
- package signatures or automatic updates;
- cross-node capability synchronization;
- RCP changes;
- dynamic libraries, WASM, hot code reload, or a service locator; or
- a universal plugin trait.

The proven base remains deliberately small: one generic Agent Plugins manager
normalizes catalog, researched MCP, and local package sources; installs their
immutable content; imports standard package skills; and connects supported
remote MCP entries through the existing deferred registry. Standalone Agent
Skills keep their global/workspace path and higher precedence. Everything runs
through Alpha and the ordinary kernel tool boundary.
