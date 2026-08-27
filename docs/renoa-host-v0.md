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
`renoa.coding.alpha.v1`.

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

## Full-access first slice

Permission semantics are deliberately open. V0 does not introduce roles,
levels, grants, approval records, or a permission trait.

The local coding profile is all-allowed. Every tool registered by its local
workspace provider is advertised directly. External catalogs are reached
through three fixed registry tools so catalog size does not become model
context. The current top-level set is:

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

`LocalHost` owns the provider configuration, durable data root, MCP catalog,
and credential resolution boundary. `host.sqlite3` keeps direct integration and
connection identities, non-secret credential references, complete catalog
snapshots, rejected entries, and Alpha's attached connection identities.
Registration, discovery, and profile attachment remain separate states.
Catalog replacement and attachment are transactional, and multi-query reads use
one SQLite snapshot so a registry call cannot observe half of a refresh.
`AlphaSession` owns one Agent/Session binding, canonical workspace, model
catalog, durable model selection, and active-turn coordination. ACP sees these
Host types; it does not construct a kernel `Runtime` or persist Host state.

The Host adds three fixed extension-registry tools to every Alpha runtime:
`tool_search`, `tool_load`, and `tool_execute`. Search returns at most five
compact matches without schemas. Load returns only one through three explicitly
requested model-facing schemas. Execute resolves one exact reference containing
the current catalog digest, then reuses the proven MCP credential, adapter,
result, and `NeverReplay` boundary. A missing adapter fails execution visibly;
it does not prevent Alpha from starting or hide searchable catalog state.

The registry tools open current `host.sqlite3` state for each call. A committed
connection attachment or catalog refresh is therefore visible on the next
search even when the ACP process, Alpha session, and current turn are already
running. The runtime itself is unchanged: the kernel freezes the same three
registry implementations, while exact references prevent a newer catalog from
silently changing a selected invocation.

`LocalRuntimeConfig` is the lower composition input used inside the Host. Every
local product path selects Alpha's versioned instructions. The resolved inputs
are:

- provider and model;
- reasoning configuration;
- Alpha's base prompt and bounded workspace `AGENTS.md` instructions; and
- the six workspace tools plus the three fixed extension-registry tools.

`build_local_runtime` resolves that recipe with a `LocalWorkspace`:

```text
LocalRuntimeConfig + Alpha v1
  + BridgeModel
  + CompactingContextStrategy
  + LocalWorkspace tools
  + Host extension registry tools
            |
            v
renoa-agent-loop::build_runtime
            |
            v
renoa-kernel::Runtime + frozen RuntimeManifest
```

The process adapter is both the model implementation and the deterministic context sizer. The
Host derives the same researched compaction limits used by the existing local
product path. Model identity, reasoning, context behavior, instructions,
limits, tool specifications, recovery declarations, and workspace-bound tool
revisions are represented by the resulting manifest.

Model and reasoning selection are not Alpha's identity. They may change
between operations while the Agent Instance, Session, instructions, tools, and
history remain continuous. A change never mutates an active operation; the
kernel freezes each operation's exact model and reasoning revision.

The Host resolves a fresh runtime for every newly admitted operation. This
re-reads the canonical workspace `AGENTS.md`, so a project-rule edit applies to
the next turn without restarting the surface. It cannot change an operation
that is already admitted because the kernel has frozen that operation's
manifest.

This recipe is not yet a durable general profile schema. Persistence should be
added only when the first management flow consumes it.

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
  -> LocalHost creates or loads AlphaSession
  -> AlphaSession accepts one caller-identified command
       -> read current workspace rules
       -> resolve the selected model, context, loop, and tools
       -> LocalSession atomically admits the command
       -> drive that exact operation through the kernel
       -> project a durable assistant or compaction result
```

`Kernel::submit_exclusive` combines the unfinished-operation check and command
insert in one immediate SQLite transaction. Alpha uses this optional admission
primitive because one conversation turn must finish before another begins.
The general kernel `submit` path still permits ordered queues for future
profiles. Exact redelivery remains idempotent; a different command is rejected
without leaving ghost queued work.

`LocalSession` remains the lower shared command boundary used by Alpha and the
headless diagnostic runner. Its prompt and explicit-compaction methods share
the same exclusive admission, stable command identity, drive, cancellation,
and durable replay path. `LocalTurnOutcome::Compacted` carries the persisted
post-compaction input estimate without pretending that a control operation
produced an assistant message. `AlphaSession` is the complete surface-facing
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
cannot be proven. It therefore makes one explicit kernel abandonment decision:
the loop records an honest unavailable tool result where applicable, the
operation fails without replaying the effect, and the next turn can proceed.
This is Host policy using the kernel's explicit boundary; the kernel itself
never abandons uncertainty automatically.

Local Host state has one intentionally visible layout:

```text
<data-root>/
  host.sqlite3                  Host MCP integrations, non-secret connection
                                references, catalogs, and Alpha attachments
  sessions/<session-uuid>/
    session.json                durable identity and workspace/profile binding
    runtime.jsonl               acknowledged provider/model/reasoning selections
    kernel.sqlite3              authoritative execution and recovery truth
    trace.sqlite3               ordered Host/model/tool diagnostics
```

Usage, cache counts, timings, provider payloads, streamed chunks, and tool
diagnostics belong in `trace.sqlite3`, never `runtime.jsonl` or model context.
Trace rows explain execution but never decide replay or semantic history.

The Host assembles these files in a hidden directory. After all four are synced
and the kernel lease is closed, it atomically renames that directory to the
session UUID and syncs the parent directory. Initialization failure removes the
staging directory, so a loadable session is never partially published. On Unix,
the published session directory is owner-only because trace and history contain
prompts, source text, and tool data.
The global `host.sqlite3` catalog is also owner-only on Unix.
`runtime.jsonl` recovery truncates an incomplete crash tail before any later
append; future valid records can never be joined onto torn JSON.

## Agent-driven changes

The full intended extension lifecycle and its staged proof plan are recorded in
[`renoa-extensions-north-star.md`](renoa-extensions-north-star.md).

The GUI is a surface, not the sole controller. A future human action or agent
tool call will issue the same typed Host management command:

```text
human surface --\
                 -> Host change command -> authorization -> durable change
running agent --/
```

An authorized agent may install, attach, update, or remove capabilities without
requiring sidebar interaction. An agent may exercise delegated authority but
must never grant itself greater authority. Capability changes produce a new
runtime for a new operation boundary; they never mutate an active manifest.

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

## Open decisions

- general profile and package-library storage beyond the first MCP attachment;
- future Host schema migrations beyond the proven v1/v2-to-v3 migrations;
- historical resolved-binding retention across explicit catalog/profile
  changes for unfinished-operation recovery;
- profile inheritance and Agent Instance overrides;
- permission vocabulary, scopes, approvals, and secret grants;
- capability package discovery, installation, updates, and rollback;
- the typed Host management command set;
- the Host management transport and presentation;
- whether capability changes pause and continue a task through one or more
  internal operations; and
- process placement for multiple concurrent local Agent Instances.

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
`AlphaSession`, creates and reloads Alpha identities, admits stable turn IDs,
streams transient model and tool events, durably cancels active effects, and
projects final answers from semantic history. Exact redelivery is proven both
within one process and after restart. Concurrent admission cannot leave ghost
work, pre-cancelled turns are not admitted, unknown effects do not wedge the
session, project instructions refresh per turn, session publication is atomic,
and torn runtime logs remain appendable. Per-turn trace rows preserve ordered
model/tool flow without entering kernel truth. The legacy harness crate is
retired.

The same Host path now admits explicit compaction as a typed control operation.
Its summary, checkpoint activation, result projection, exact redelivery,
cancellation, and post-restart usage restoration are kernel-backed; no surface
owns or reconstructs that state.

The first extension path is also complete. `LocalHost` registers direct no-auth
or exact `gh`-referenced MCP connections, runs the replaceable Node adapter for
bounded discovery and invocation, atomically publishes catalogs and tool
attachments, and restores them after process restart. Alpha exposes three fixed
registry tools regardless of catalog size. Search and load are bounded
`SafeToReplay` reads; execute carries an exact catalog reference through the
normal loop and kernel as a `NeverReplay` effect. Exact registration retries
converge, identity changes conflict, failed refresh publication preserves the
previous snapshot, stale references fail closed, structured details stay
outside model context, unknown calls are not replayed, and schemas v1 and v2
migrate to v3 without losing catalog state. A live registry object observes a
newly committed attachment, and searching 1,000 tools exposes no schema. No
kernel type or table changed.
