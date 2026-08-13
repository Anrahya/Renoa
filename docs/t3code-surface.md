# T3 Code surface fork

## Status

T3 Code is the application base for Renoa's first frontend surface. Renoa uses
the upstream source directly and carries a small integration patch set rather
than recreating its UI or maintaining a one-time code copy.

Pinned import revision:
`2db08457f2f4eaaa713a067b2ea480ca2b583025`.

Upstream repository: <https://github.com/pingdotgg/t3code>.

License: MIT, copyright T3 Tools Inc. Preserve the upstream `LICENSE` and
applicable third-party notices in distributed source and binaries.

## Repository layout

The local `t3code/` directory is a separate Git checkout ignored by the Renoa
core repository. Its `upstream` remote points to `pingdotgg/t3code`; its
`origin` remote points to `Anrahya/t3code`; and Renoa work lives on the
dedicated `renoa/main` integration branch.

This separation keeps upstream history and merge ancestry intact. Do not copy
the T3 tree into the Renoa core Git history.

## Integration boundary

Do not put RCP between T3 and the first local Renoa harness. Prove the harness
boundary first through T3's existing ACP provider seam:

`T3 clients -> T3 server -> Renoa ACP driver -> ACP/stdio -> Renoa agent`

This is the smallest path that proves process lifecycle, prompts, streaming,
cancellation, permissions, and session state against the real Rust executable.
The frontend remains unaware of the agent implementation.

RCP is a later, sibling integration for durable continuity:

`T3 surface adapter -> RCP -> Renoa node adapter -> same agent core`

Locked rules:

- ACP and RCP are sibling adapters over the same harness core. RCP does not wrap
  ACP and ACP does not become part of the RCP contract.
- RCP remains authoritative only on the continuity path: durable tasks,
  commands, events, routing, acknowledgement, replay, and delivery recovery.
- Provider-native configuration and permissions remain outside the RCP kernel.
- T3 persistence may hold surface state and idempotent projections, but cannot
  redefine an RCP admission or execution outcome.
- Stable mappings and identities must make RCP replay safe to apply more than
  once.

The Renoa ACP driver should wait only for a minimal executable vertical slice
from the Rust SDK. The rest of this surface can be prepared and validated now.

The T3 side of that slice is now present and disabled by default. Its provider
settings launch `renoa-agent acp` unless overridden. The adapter covers standard
ACP new/load session, prompt streaming, stable prompt identity, cancellation,
permissions, image attachments, resume, and T3's browser MCP handoff. It does
not import Renoa Rust crates or RCP types.

## Audit findings

The retained application boundary is sound. T3's web, desktop, server,
client-runtime, contracts, orchestration, Markdown/activity rendering, diff and
terminal tooling, sidebar, preview, and generic ACP implementation remain the
surface base. A Renoa harness integration belongs behind T3's provider-driver
interface; it does not require a parallel frontend or a second browser RPC
stack.

The first surface-fork cut intentionally excludes marketing, mobile, relay, and
release-announcement packages from Renoa's normal build and test commands. The
source directories remain in the upstream-tracking checkout because they do not
enter the desktop artifact and deleting them would create recurring merge work.
Upstream PostHog analytics and automatic updates are disabled by default and
require explicit configuration to run.

The existing provider drivers remain temporarily load-bearing. Remove them only
after the Renoa ACP executable completes the local prompt, stream, cancel, and
resume lifecycle; until then they are the regression harness for the UI and
provider seam.

The surface shell has low adaptation cost. One product-lifecycle prerequisite
for a later RCP adapter remains and must not be hidden with a local guess:

1. T3 starts a new local thread, while RCP v0 only lists and attaches to an
   already-provisioned durable task. The first product flow must explicitly
   choose between importing an existing RCP task and admitting a new one.

The audit also found that the prior RCP execution task record did not identify
its causing command. Binding version 8 now carries that stable causation, the
coordinator migrates existing records without changing their identities or
sequences, and the TypeScript surface decoder rejects records that omit it.

Cancellation, steering, approvals, rollback, and transient token deltas are
also absent from RCP v0. A driver must advertise or reject those capabilities
honestly until a real Renoa execution path defines them.

Do not implement a driver that binds every new T3 thread to one configured task,
chooses a task by list order, correlates an execution with the latest command,
or bypasses RCP to create an agent session. Those shortcuts break durable task
identity or replay correctness.

Recommended sequence:

1. Implement the Renoa ACP executable against the already-registered narrow T3
   provider driver.
2. Prove local new thread, prompt, stream, cancellation, permission, and resume
   behavior end to end.
3. Define a separate RCP surface adapter and node adapter around the same agent
   core.
4. Add task admission and attachment only after routing and node-side
   provisioning policy is defined outside the harness-neutral RCP kernel.

## Upstream compatibility

- Prefer new Renoa modules and narrow registry changes.
- Avoid mass formatting, directory moves, or broad renames in upstream files.
- Keep upstream tests intact and add deterministic tests around Renoa seams.
- Merge upstream as a normal Git ancestor; do not manually recopy changed UI
  files.
- Record the new upstream base revision after every sync.
- Deliberate divergence is allowed when it serves Renoa. Document why a broad
  divergence is worth its ongoing merge cost.

Unmodified upstream modules are exempt from Renoa's 500-line limit. New
Renoa-owned modules are not.
