# Renoa extension system north star

## Status and authority

This document defines the intended product and architecture direction for
installing, connecting, selecting, and using replaceable Renoa capabilities.
It is the north star for extension work, not yet a storage schema, wire
protocol, permission model, or public package-registry contract.

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
  -> Host inspects an exact source
  -> Host produces an immutable installation and connection plan
  -> an authorized decision approves or rejects that plan
  -> Host installs the package and establishes any required connection
  -> selected components become available to selected agent profiles
  -> the next operation freezes and uses the new runtime
```

The agent may find, inspect, and request a capability through the same Host
semantics used by the GUI. It must not treat its own request as approval to
increase its authority. The GUI is a convenient surface for understanding and
approving changes, not the sole controller of Renoa.

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
  -> plan approved
  -> package installed immutably
  -> integration definition registered
  -> connection configured/authenticated
  -> component catalog discovered
  -> profile binding selected
  -> runtime resolved for the next operation
  -> runtime frozen by the kernel
```

Installation does not imply a connection. A connection does not imply profile
selection. Profile selection does not bypass authorization. Catalog discovery
does not make every schema part of model context. None of these mutable Host
states changes an already admitted operation.

## End-to-end management flow

### Inspect

The Host resolves an exact local or remote source without executing package
code. It validates the package boundary and manifest version, records source
and license evidence, and enumerates the files and components it understands.
Installing a package never grants permission to execute its scripts or start
its servers.

### Plan

The Host produces an immutable content-bound plan showing at least the package
identity and digest, source revision, supported and unsupported components,
commands or executables, network endpoints, requested connection inputs, and
the profile changes requested by the caller. Exact plan fields remain a later
contract decision.

### Approve and install

Approval targets that exact plan. Installation copies package contents through
a staging location into an immutable content-addressed store, then durably
publishes the installed record before acknowledging success. A changed source
requires a new plan and approval.

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

A profile chooses components by stable identity. The Host applies current
scope and future policy, resolves the concrete adapters, and builds ordinary
`AgentToolBinding` or other native bindings. Duplicate or unrepresentable
model-visible names fail before command admission rather than silently hiding a
tool.

### Execute

The kernel freezes exact component and adapter revisions through the existing
runtime manifest and configuration digest. External MCP calls pass through the
same intent-before-effect boundary as native tools.

### Update, disable, and remove

An update installs another immutable digest; it does not rewrite the prior
package in place. Profile changes apply at the next operation. Removal cannot
delete content still required to recover an unfinished operation. Rollback is
selection of a previously installed compatible digest, not restoration from
mutable leftovers.

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
11. An active operation keeps its exact frozen runtime even if packages,
    connections, catalogs, or profile selections change.
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

1. the profile's instructions, a bounded index of eligible skills, and only
   deliberately activated full skill content;
2. the durable context projection; and
3. the effective tool definitions required for that operation.

Package manifests, marketplace descriptions, setup instructions, connection
state, OAuth scopes, environment variables, secret references, process
bookkeeping, schema hashes, and runtime manifests remain outside model context.
Tool failures that help the model recover are returned as bounded model-visible
tool results; operational diagnostics and sensitive details remain in Host
trace data.

Profile selection makes a skill eligible; it does not require Renoa to inject
every selected skill in full. The exact activation and lazy-loading mechanism
remains open until the first skill vertical slice proves it.

## Physical ownership

The intended physical separation is:

```text
Renoa repository
  crates/renoa-local/          Host library, assembly, and management semantics
  adapters/mcp-client-node/    replaceable MCP protocol implementation

renoa-plugins repository
  official/<plugin>/           Renoa-owned packages
  third_party/<plugin>/        packages for external services

Renoa data directory
  installed package store      immutable package digests
  Host catalog                 installations, integrations, connections,
                               discovered components, and profile bindings
  sessions/<session>/          existing kernel and trace truth

credential store
  secret material keyed by opaque Host references
```

This north star does not lock the broader Host database or secret-store design.
The direct MCP v0 slice now consumes `host.sqlite3` for its integration,
connection, catalog, and Alpha-selection records; later package, permission,
credential, and migration shapes remain open.

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

Reviewed on 2026-08-27. No upstream source is copied by this documentation
slice.

| Source | Exact revision | License evidence | Renoa use |
| --- | --- | --- | --- |
| [Agent Plugins 1.0](https://github.com/agentplugins/agent-plugins-spec/tree/ff8ab5e392cc87bd88d87c060815a87490e51003) | `ff8ab5e392cc87bd88d87c060815a87490e51003` | Specification and docs CC-BY-4.0; schemas and software Apache-2.0 | Portable package, skill, MCP, containment, discovery, and client-extension floor |
| [Agent Skills](https://github.com/agentskills/agentskills/tree/69ef37e9424c0a7ea9dd2293b559e43ec8176379) | `69ef37e9424c0a7ea9dd2293b559e43ec8176379` | Code Apache-2.0; documentation CC-BY-4.0 | Portable `SKILL.md` structure and progressive-disclosure model |
| [MCP 2026-07-28](https://github.com/modelcontextprotocol/modelcontextprotocol/tree/5f5440bb26a62e2cf3440b92da5a667efa03b267) | tag commit `5f5440bb26a62e2cf3440b92da5a667efa03b267` | Repository records an Apache-2.0 transition with remaining MIT material and CC-BY-4.0 documentation | Current remote tool protocol semantics |
| [MCP TypeScript client 2.0.0](https://github.com/modelcontextprotocol/typescript-sdk/tree/cc4b41617ce3601b1290d67216ea0b194a3cd9ac) | tag commit `cc4b41617ce3601b1290d67216ea0b194a3cd9ac` | Published package declares MIT; source repository records the broader MCP license transition | Maintained implementation behind Renoa's narrow process adapter |
| [Cursor plugins](https://github.com/cursor/plugins/tree/bdf7aa355337897f167153e05069aca505dae17c) | `bdf7aa355337897f167153e05069aca505dae17c` | MIT at reviewed revision | One-directory-per-package marketplace organization; Cursor-specific manifests are not Renoa's portable contract |
| [Executor](https://github.com/UsefulSoftwareCo/executor/tree/7c12aeea390225291ce4c97865b392237ee7934d) | `7c12aeea390225291ce4c97865b392237ee7934d` | MIT | Evidence for separating integrations, authenticated connections, discovered tools, and just-in-time secret resolution |

Agent Plugins 1.0 standardizes only skills and MCP server entries. It explicitly
leaves registries, installation, permissions, trust, caching, and product UX to
the client. Renoa owns those responsibilities in the Host rather than extending
the portable manifest with accidental product policy.

MCP 2026-07-28 removes protocol-level sessions and makes each request
self-describing. The official TypeScript SDK v2 supports the revision, but
modern version negotiation is an explicit client choice. Renoa pins the modern
revision and tests that an older-only endpoint fails without fallback rather
than assuming that installing v2 changes the wire automatically.

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
6. Adding or removing a component changes only future operation manifests.
7. A package author uses the Agent Plugins schemas rather than a Renoa-only
   layout for portable skills and MCP servers.
8. Invalid, incompatible, or unsupported components fail independently where
   the standard permits, with exact diagnostics.
9. The user can understand source, code, endpoints, requested access, selected
   profiles, and expected changes before approval.
10. An authorized agent can drive the same inspect-and-request flow as the GUI
    without modifying Host or kernel files directly.

## Implementation sequence and proof gates

Each slice must leave the repository coherent and pass its proof gate before
the next begins.

Slices 1 through 3 are complete. Slice 4 is next.

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
errors. Do not add auth, stdio, resources, prompts, apps, or tasks.

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

Resolve one selected MCP tool into the existing `renoa-agent::Tool` and
`AgentToolBinding` path. Use a deterministic model and local MCP server to run
one complete Alpha operation through the real kernel.

Proof gate: only the selected schema reaches the model, exact revisions are
frozen, restart does not duplicate a settled call, a possibly dispatched call
is never replayed, schema drift affects only a future operation, and useful
tool errors return to the model.

### 5. Agent Plugins local loader

Load and validate one local Agent Plugins 1.0 directory using locally pinned
schemas. Enforce package containment and component-level failure boundaries,
then publish an immutable content-addressed installation. Map its MCP entry onto
the already proven integration path.

Proof gate: malformed manifests, unsupported versions, path escapes, changed
contents, crash during publication, and independent component failures are
deterministic and leave no partially installed package.

### 6. Authenticated connections and public packages

Add one credential-backed connection, followed by OAuth only when its full
flow has a real service consumer. Create the separate `renoa-plugins`
repository and publish reviewed Agent Plugins packages there. Installation
continues to pin exact source and content identity.

Proof gate: secrets never enter package or model-visible data, concurrent or
crashed auth cannot corrupt rotating credentials, multiple accounts remain
isolated, and a catalog package adds no special-case core execution code.

### 7. Shared Host management

Define the typed Host management commands consumed by both Waku and an agent
tool. The agent may inspect and request; an authorization boundary commits the
exact approved plan. The resulting capability becomes available only at the
next operation boundary.

Proof gate: GUI and agent flows reach the same durable state transition, lost
replies are idempotent, changed plans require renewed approval, and the agent
cannot approve or broaden its own request.

### 8. Later fabric work

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
12. A request to extend Renoa is not approval to expand authority.
13. Capability changes affect only future operations; the kernel freezes the
    active operation's exact runtime.
14. Only effective instructions and tool definitions enter model context.
15. Secrets remain behind a dedicated credential boundary and are referenced,
    not copied, by Host records.
16. Possibly dispatched external calls are not automatically replayed without
    a proven idempotence contract.
17. RCP remains continuity and placement infrastructure, not extension
    execution or credential distribution.
18. Every slice is proved through the real boundary it introduces before the
    next layer is built.

## Open decisions

- Host schema migrations and storage for packages, credentials, and permissions;
- model-visible naming and collision handling for tools from many connections;
- exact catalog freshness, cache-hint, and refresh policy;
- secret-store implementation and account-recovery behavior;
- OAuth client metadata, browser redirect, and headless-device flows;
- permission vocabulary, scopes, policy inheritance, and approval records;
- profile persistence, inheritance, and Agent Instance overrides;
- exact Host management command types and transport;
- registry index, search, signing, trust, update, and review policy;
- package garbage collection and unfinished-runtime retention policy;
- when to add stdio MCP servers and how to constrain their process and
  filesystem authority;
- skill selection, lazy loading, context budgets, and conflict resolution;
- support for MCP resources, prompts, apps, tasks, or future protocol
  extensions;
- compatibility policy for pre-2026 MCP servers;
- process lifetime and multiplexing for the MCP adapter;
- node capability advertisement and cross-node Host configuration; and
- stronger service-specific idempotency, reconciliation, or callback contracts.

These are not permission to postpone the first vertical proof. They are
boundaries against guessing beyond it.

## First-slice non-goals

The first direct MCP slice does not implement:

- package registries or marketplace browsing;
- Agent Plugins loading;
- OAuth, API keys, or secret storage;
- stdio MCP servers;
- MCP resources, prompts, apps, tasks, sampling, or elicitation;
- Waku settings or approval UI;
- agent-driven installation;
- a general permission system;
- package signatures or automatic updates;
- cross-node capability synchronization;
- RCP changes;
- dynamic libraries, WASM, hot reload, or a service locator; or
- a universal plugin trait.

The first success is deliberately smaller: one selected remote tool, resolved
by the Host, invoked through MCP, and durably completed through the existing
Alpha and kernel path.
