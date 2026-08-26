# Reference implementations

Renoa is a new implementation with its own contracts. The following upstream
projects are studied for established agent-runtime mechanics.

## Pi

- Repository snapshot: `a96fb984d8c8b065fc5d193309fc812a882adee0`
- Cancellation follow-up: `496185f6e4267b979e3663c45f7eb70b0c6a97b4`
  (MIT)
- Remote-catalog follow-up: `9d2ec7ffabe927bfad2214c1cee25b6632a78dcf`
  (MIT)
- Runtime packages: `@earendil-works/pi-agent-core@0.84.2` and
  `@earendil-works/pi-ai@0.84.2`, published from
  `914cf1472e715297caa30db4b9535d534a9eb718`
- License: MIT
- Studied source: `pi/packages/agent/src/agent-loop.ts`,
  `pi/packages/agent/docs/harness-v2.md`, `pi/packages/ai`,
  `pi/packages/coding-agent/src/core/remote-catalog-provider.ts`,
  `pi/packages/protocol`, and `pi/packages/server`.
- Adopted design evidence: a small model-to-tool continuation loop,
  source-ordered content and tool-result reinjection, indexed streaming,
  definite tool failures as model-visible results, rich progress, normalized
  usage, turn-wide cancellation, and separation of authoritative state from
  transient connection progress. Renoa adds mandatory kernel persistence,
  exact effect identities, and honest unknown outcomes rather than copying
  Pi's in-memory orchestration.
- Current RCP proof: `nodes/pi` uses the pinned Pi packages directly, projects
  only durable activity into RCP, and keeps Pi messages, provider events,
  workspace policy, and credentials local to that node. Pi's CLI, TUI, and
  coding-agent package are not dependencies.
- Current Alpha provider path: `adapters/model-provider-node` is Renoa-owned and
  has no Pi runtime-package dependency. It adapts the minimal xAI and OpenCode
  Go provider source needed from Pi `v0.84.2`; exact copied files, removals,
  modifications, source revision, and MIT notice are recorded beside the code
  in `adapters/model-provider-node/UPSTREAM.md`.
- Renoa deliberately does not adopt Pi's catalog persistence, unbounded queues,
  hidden inference retries, or unfinished durable-harness proposal. Context
  projection, compaction, recovery, and scheduling remain Renoa contracts.

## OpenAI Codex CLI

- Repository snapshot: `02bc1dd796e367619b44fe62825d9f118470ad6f`
- Cancellation follow-up: `3b45c29062ff0e76e71c91b6753290400e7fa8da`
- License: Apache-2.0
- Studied source: `codex/codex-rs/core/src/session/turn.rs` and
  `codex/codex-rs/app-server-transport/src/transport/remote_control`
- Adopted ideas: explicit turn cancellation, event-driven execution,
  turn-scoped model state, bounded continuation, and separation between model
  sampling and tool execution. The follow-up confirmed exact thread-and-turn
  targeting, an explicit interrupted terminal state, and draining in-flight
  tool work before cancellation returns. The continuity proof also adopts
  outbound node connections, stable identities independent of a socket,
  bounded outbound queues, sequence-based recovery, and idempotent redelivery.
  It does not copy Codex's service-specific enrollment or relay protocol.

## Grok Build

- Repository snapshot: `ed6d543643628663873c5de28298e022ed634238`
- Cancellation follow-up: `19d42e35c07a9c9244f03f6df0c4c353f970d4f9`
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
  completed; only later sequential calls are recorded as not run. The
  cancellation follow-up confirmed exact active-prompt targeting and an
  explicit cancelled terminal state. Renoa does not copy Grok Build's actor or
  hard-abort machinery.

## DeepSeek Harness

- Repository snapshot: `99f6f02fecdb7dff40c3fbc9470f5907c29f74ca`
- Cancellation follow-up: `141eb6fef83422698aef7a981029e843e8161534`
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
  after closing the transcript. No DeepSeek source is copied. The cancellation
  follow-up confirmed a turn-wide cooperative signal, quiescence before
  reporting completion, and the useful distinction between work aborted before
  dispatch and work that may have run. Renoa preserves those facts in its
  durable effect journal rather than copying DeepSeek's runtime implementation.

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

The Pi, Codex, Grok Build, and DeepSeek Harness repositories above are studied
references. Substantial Pi provider code adapted into Renoa is recorded with
its exact source revision, modification scope, and license in
`adapters/model-provider-node/UPSTREAM.md`; the other reviewed source is not
copied wholesale.

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
