# Reference implementations

Renoa is a new implementation with its own contracts. The following upstream
projects are studied for established agent-runtime mechanics.

## Pi

- Repository snapshot: `a96fb984d8c8b065fc5d193309fc812a882adee0`
- Remote-catalog follow-up: `9d2ec7ffabe927bfad2214c1cee25b6632a78dcf`
  (MIT)
- Runtime packages: `@earendil-works/pi-agent-core@0.84.1` and
  `@earendil-works/pi-ai@0.84.1`, published from
  `53fa77ccd8a279eb87e92294ef3687b03ff80112`
- License: MIT
- Studied source: `pi/packages/agent/src/agent-loop.ts`,
  `pi/packages/agent/docs/harness-v2.md`,
  `pi/packages/agent/src/agent.ts`,
  `pi/packages/ai/src/types.ts`,
  `pi/packages/coding-agent/src/core/remote-catalog-provider.ts`,
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
  The standalone Rust coding host also calls Pi AI through a one-request local
  process adapter. Pi performs provider authentication and wire translation;
  Renoa's Rust harness remains the only conversation and tool loop.
  Renoa's durable projector also commits its exact model request before
  dispatch; Pi's in-memory `transformContext` does not provide that recovery
  boundary. For xAI models newer than the installed Pi AI package, the local
  adapter uses Pi's official live-catalog endpoint but accepts only a selected
  record with the expected provider, supported API, and trusted xAI base URL.
  The Rust host pins that exact record across its Node subprocesses and puts its
  SHA-256 identity in the runtime revision. Pi's shipping loop treats ordinary
  tool exceptions as results; its separate harness-v2 design proposes replaying
  safe effects and inserting interrupted results for unsafe ones. Renoa adopts
  the recovery distinction, not that unimplemented design or its code. Renoa
  did not copy Pi's catalog provider or its persistence layer.

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
- Studied source: `xai-grok-agent`, `xai-grok-sampler`, `xai-chat-state`,
  `xai-grok-sampling-types/src/conversation.rs`, the shell turn and tool-call
  pipeline, and `WorkspaceOps`.
- Adopted ideas: resolve a declarative agent before binding it to a run, keep
  sampling reliability outside the continuation loop, and treat local or
  remote capability execution as a host concern rather than a surface concern.
- Adopted recovery idea: close every unresolved call in source order and make
  repair idempotent. Renoa deliberately does not copy Grok Build's blanket
  statement that a dangling call was not executed, because a crash after
  dispatch cannot prove that claim. The current call is recorded as possibly
  completed; only later sequential calls are recorded as not run.

## DeepSeek Harness

- Repository snapshot: `99f6f02fecdb7dff40c3fbc9470f5907c29f74ca`
- Release at review: `dsh-0.1.0-rc.7` developer preview
- License: MIT
- Studied source: Cordis runtime and plugin contracts, session and agent event
  boundaries, bundled profiles, `session-checkpoint-policy`, and
  `packages/core/session/src/repair.ts`.
- Adopted ideas: typed replaceable capability seams, runtimes assembled from
  named plugins instead of an agent-kind hierarchy, per-agent scope, and a
  clear distinction between durable session facts and transient live events.
  Renoa makes a different foundational choice: persistence, ordering, frozen
  runtime identity, and generic effect recovery are mandatory kernel laws, not
  optional plugins. DeepSeek's checkpoint policy persists before model and
  top-level tool calls but can still recover an interrupted tool only as
  unknown; Renoa generalizes that boundary to every external effect and records
  intent separately from possible dispatch. Renoa also adopts DeepSeek's useful
  distinction between a tool that never started and one whose outcome is
  unknown. Unlike DeepSeek's automatic interrupted-turn repair, Renoa preserves
  the block until an explicit host action and keeps the external effect unknown
  after closing the transcript. No DeepSeek source is copied.

## Durable workflow systems

Reviewed on 2026-08-20.

- [Temporal activity retry](https://github.com/temporalio/documentation/blob/main/docs/encyclopedia/retry-policies.mdx)
  and [asynchronous completion](https://github.com/temporalio/documentation/blob/main/docs/encyclopedia/activities/activity-execution.mdx),
  [AWS Step Functions callback task tokens](https://docs.aws.amazon.com/step-functions/latest/dg/connect-to-resource.html),
  and [Restate awakeables](https://docs.restate.dev/develop/go/external-events)
  were reviewed for the general external-effect boundary.
- Adopted rule: once dispatch may have happened and the reply is lost, a caller
  cannot infer the external result. It may retry only with an idempotence
  guarantee, accept an authoritative result tied to a stable identity, or keep
  the outcome unknown until an explicit decision.
- Renoa's abandonment path implements the third option. The existing stable
  `EffectId` is sufficient for a future callback or status lookup, so no unused
  receipt, token, or reconciliation identifier is added now. No workflow-system
  source is copied.

No source file is copied wholesale. If Renoa later incorporates substantial
upstream code, the relevant license notice and modification history will be
preserved with that code.

## Interoperability standards checked

Last reviewed on 2026-08-14:

- [Agent Client Protocol](https://agentclientprotocol.com/) stable wire version
  1 is the coding-frontend boundary. Renoa uses
  `agent-client-protocol@2.0.0` without unstable features, from Rust SDK commit
  `07926d7f9468e149e4fb676ab531b410aa8143cb` (Apache-2.0). The crate release
  number is not the wire version. Draft protocol v2, remote transports, and
  multi-client replay are outside the first adapter. ACP does not replace
  Renoa's durable harness or RCP continuity journal.
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
