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
outside this core repository and connect through ACP; the first graphical
surface is a Rust/GPUI Zeron fork. ACP is the standard agent-facing surface
protocol. Renoa-specific capability management will use a separate logical Host
API whose transport is not selected until a real management consumer is
implemented.

The first concrete agent profile is Renoa Alpha v1, specified in
[`renoa-alpha-v1.md`](renoa-alpha-v1.md). Its stable Host identity is
`renoa.coding.alpha.v1`.

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
change can affect a later operation but cannot mutate the active one.

Profiles are declarative recipes. They do not execute effects and do not own
session history. Installed availability does not imply future authorization;
that distinction remains required even though v0 intentionally has no
permission system.

## Full-access first slice

Permission semantics are deliberately open. V0 does not introduce roles,
levels, grants, approval records, or a permission trait.

The local coding profile is all-allowed: every tool registered by its local
workspace provider is advertised to the model and bound into the runtime. The
current set is:

```text
read_file
edit_file
write_file
bash
grep
find
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

`LocalHost` owns the provider configuration and durable session root.
`AlphaSession` owns one Agent/Session binding, canonical workspace, model
catalog, durable model selection, and active-turn coordination. ACP sees these
Host types; it does not construct a kernel `Runtime` or persist Host state.

`LocalRuntimeConfig` is the lower composition input used inside the Host. Every
local product path selects Alpha's versioned instructions. The resolved inputs
are:

- provider and model;
- reasoning configuration;
- Alpha's base prompt and bounded workspace `AGENTS.md` instructions; and
- the concrete credential and adapter bindings.

`build_local_runtime` resolves that recipe with a `LocalWorkspace`:

```text
LocalRuntimeConfig + Alpha v1
  + BridgeModel
  + CompactingContextStrategy
  + LocalWorkspace tools
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

The first real Host flow is:

```text
surface adapter or local caller
  -> LocalHost creates or loads AlphaSession
  -> AlphaSession accepts one caller-identified turn
       -> read current workspace rules
       -> resolve the selected model, context, loop, and tools
       -> LocalSession atomically admits the command
       -> drive that exact operation through the kernel
       -> project durable semantic output
```

`Kernel::submit_exclusive` combines the unfinished-operation check and command
insert in one immediate SQLite transaction. Alpha uses this optional admission
primitive because one conversation turn must finish before another begins.
The general kernel `submit` path still permits ordered queues for future
profiles. Exact redelivery remains idempotent; a different command is rejected
without leaving ghost queued work.

`LocalSession` remains the lower shared command boundary used by Alpha and the
headless diagnostic runner. `AlphaSession` is the complete surface-facing Host
boundary: it also owns runtime selection, persistence, fresh per-turn
composition, and cancellation coordination.

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

New session state has one intentionally visible layout:

```text
sessions/<session-uuid>/
  session.json     durable identity and workspace/profile binding
  runtime.jsonl    acknowledged provider/model/reasoning selections
  kernel.sqlite3   authoritative execution and recovery truth
  trace.sqlite3    ordered Host/model/tool diagnostics
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
`runtime.jsonl` recovery truncates an incomplete crash tail before any later
append; future valid records can never be joined onto torn JSON.

## Agent-driven changes

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

- durable profile and capability-library storage;
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
