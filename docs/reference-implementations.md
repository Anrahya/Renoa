# Reference implementations

Renoa is a new implementation with its own contracts. The following upstream
projects are studied for established agent-runtime mechanics.

## Pi

- Repository snapshot: `a96fb984d8c8b065fc5d193309fc812a882adee0`
- Runtime packages: `@earendil-works/pi-agent-core@0.84.1` and
  `@earendil-works/pi-ai@0.84.1`, published from
  `53fa77ccd8a279eb87e92294ef3687b03ff80112`
- License: MIT
- Studied source: `pi/packages/agent/src/agent-loop.ts`,
  `pi/packages/agent/src/agent.ts`,
  `pi/packages/ai/src/types.ts`,
  `pi/packages/protocol`, and `pi/packages/server`
- Adopted ideas: the minimal model-to-tool continuation loop, a stateful Agent
  around that reusable loop, awaited ordered lifecycle events, source-ordered
  content blocks and tool-result reinjection, block-indexed streamed text with
  explicit aborts, representing tool failures as model-visible results,
  successful stop reasons, normalized per-response token usage, continuation
  from an existing transcript, distinct steering and follow-up queues, host
  context projection, rich tool results and progress, and explicit parallel
  tool scheduling. Renoa also retains opaque provider continuation metadata
  rather than flattening it out of the portable transcript.
  Renoa keeps pricing outside conversation state and treats omitted usage as
  unknown instead of Pi's zero-filled value. Renoa's Rust SDK defaults tool
  execution to sequential, and makes queues and progress channels bounded.
  Queues remain reachable through a clonable control handle rather than copying
  Pi's unbounded Agent-owned implementation. The continuity proof also follows
  Pi's explicit version handshake and separation of authoritative stored state
  from transient connection progress. Renoa keeps JSON for v0 instead of
  adopting Pi's CBOR framing. The first external node uses Pi directly and
  projects only the durable event intersection into RCP; Pi messages and
  provider events remain local. Its local workspace adapter reuses Pi's
  packaged read, write, and edit tools and adds a target binding and
  path-confinement check rather than reimplementing Pi's file behavior. That
  tool configuration belongs to the Pi adapter, not RCP. The node also consumes
  Pi's provider-owned OAuth/refresh contract through its own durable credential
  store. Pi's CLI, TUI, and coding-agent package are not dependencies.

## OpenAI Codex CLI

- Repository snapshot: `02bc1dd796e367619b44fe62825d9f118470ad6f`
- License: Apache-2.0
- Studied source: `codex/codex-rs/core/src/session/turn.rs` and
  `codex/codex-rs/app-server-transport/src/transport/remote_control`
- Adopted ideas: explicit turn cancellation, event-driven execution,
  turn-scoped model state, bounded continuation, and separation between model
  sampling and tool execution. The continuity proof also adopts outbound node
  connections, stable identities independent of a socket, bounded outbound
  queues, sequence-based recovery, and idempotent redelivery. It does not copy
  Codex's service-specific enrollment or relay protocol.

## Grok Build

- Repository snapshot: `ed6d543643628663873c5de28298e022ed634238`
- Monorepo source revision: `d6937fe255dce4133c3d000a50f9cb94de12f06f`
- License: Apache-2.0; its tool crate carries additional notices for adapted
  Codex and OpenCode implementations.
- Studied source: `xai-grok-agent`, `xai-grok-sampler`, `xai-chat-state`, the
  shell turn and tool-call pipeline, and `WorkspaceOps`.
- Adopted ideas: resolve a declarative agent before binding it to a run, keep
  sampling reliability outside the continuation loop, and treat local or
  remote capability execution as a host concern rather than a surface concern.
- Deferred ideas: dangling-call recovery and ACP as a coding-surface adapter.
  These require runtime consumers before they belong in the kernel.

No source file is copied wholesale. If Renoa later incorporates substantial
upstream code, the relevant license notice and modification history will be
preserved with that code.

## Interoperability standards checked

Last reviewed on 2026-08-08:

- [Agent Client Protocol](https://agentclientprotocol.com/) has stabilized
  session resume and list operations. Its remote HTTP/WebSocket binding and v2
  multi-client replay work remain under development. ACP is a useful coding
  surface adapter, but its agent-owned session is not Renoa's authoritative task
  journal or execution binding.
- [AG-UI](https://docs.ag-ui.com/) provides transport-independent frontend
  events, snapshots, and deltas. It is a strong candidate for an RCP activity
  profile, but it does not define Renoa's device authority, complete cursor
  replay, or node dispatch.
- [A2A 1.0](https://a2a-protocol.org/latest/specification/) allows a task to
  outlive an individual stream and permits multiple clients to receive the same
  ordered live updates. Its task history is not required to contain every
  message missed during disconnection, and the serving agent still owns the
  task. RCP requires a complete authoritative journal owned independently of an
  agent process.
- [MCP](https://modelcontextprotocol.io/) remains a tool, resource, and context
  boundary behind an executor rather than a surface-continuity protocol.
