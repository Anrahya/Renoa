# Renoa Agent v0

## Purpose

`renoa-agent` is a standalone Rust agent SDK. It owns one conversation and the
bounded model-to-tool continuation loop. It can be embedded in a desktop app,
VPS process, execution node, or another host without importing RCP, SQLite, or
surface code.

The SDK is independent of `renoa-runtime`. The latter remains Renoa's durable
RCP reference executor; neither crate wraps the other.

## Boundary

- `Agent` owns conversation state, model continuation, tool dispatch, and run
  limits.
- `Model` is the provider-adapter port. Provider authentication, retry policy,
  and wire formats stay in adapters.
- `Tool` is the external capability port. The SDK advertises host-selected
  schemas and executes returned calls, but ships no filesystem, shell, search,
  or product tools.
- `AgentHandle` observes and aborts the active prompt without borrowing the
  `Agent`. It also accepts steering and follow-up input while the Agent is
  mutably borrowed by an active run.
- `AgentEventSink` receives awaited lifecycle events for structured model
  output and tool execution.
- `ContextProjector` lets a host choose the active transcript independently for
  every model request without rewriting authoritative Agent state.
- `AgentState` contains the portable active transcript, not the authoritative
  full session history. System instructions, tools, model selection, and policy
  are supplied again by the host after restoration. A host constructs its
  projected transcript with `AgentState::from_messages`.

One `Agent` is one conversation. A host must not share it between unrelated
tasks or principals.

## Proven behavior

The tests prove that:

1. ordered text/image user content reaches a provider-neutral `Model` with no
   Renoa protocol or storage dependency;
2. ordered lifecycle events expose text, reasoning, tool-call identity, and
   JSON-argument deltas with their content-block index, while only the
   completed model response enters conversation state;
3. failed partial streams emit `MessageAbort` and do not persist partial output;
4. an `AgentHandle` cancels an active prompt and remains busy until the final
   awaited `AgentEnd` listener settles;
5. serialized conversation state resumes under host-supplied system
   instructions, which are never serialized into that state;
6. image blocks, signed text, reasoning, tool-call signatures, response IDs,
   and provider/model identity survive JSON state restoration;
7. interleaved assistant text and tool-call blocks, plus structured tool
   results, preserve source order through execution and continuation;
8. missing tools and tool failures become model-visible error results rather
   than crashing the loop;
9. `length`-stopped calls with possibly truncated arguments never execute;
10. duplicate tool names, oversized call batches, and runaway model
   continuation are bounded explicitly, without losing reported usage from a
   rejected model response;
11. bounded live tool updates stay transient while final text/image content and
    structured details enter durable history exactly once;
12. parallel-safe tool calls may finish out of order while their result messages
    enter history in assistant source order; one sequential tool serializes the
    entire batch;
13. `resume()` retries an existing user or tool-result tail without duplicating
    it, and rejects invalid empty or completed tails with typed errors;
14. steering waits until the current assistant response and its complete tool
    batch have entered history, then takes priority over follow-ups;
15. follow-ups run only when the Agent would otherwise stop, with configurable
    one-at-a-time or all-at-once draining;
16. queued text/image input is bounded, remains queued when a run reaches its
    turn limit, is claimed atomically when resuming a completed assistant tail,
    and cannot be invalidated by lowering its configured bound;
17. `reset()` clears the transcript and queues while leaving the configured
    model and instructions usable for a fresh prompt;
18. successful provider outcomes and normalized token usage survive state
    restoration, while complete multi-turn usage is summed for the run;
19. host context projection runs before every model request without mutating the
    full in-memory transcript; and
20. model, instructions, and tools can change safely between runs, with an
    invalid tool replacement leaving the previous set intact.

Tool calls execute sequentially by default. A host may enable parallel batches;
any configured tool can still declare itself sequential and force the whole
assistant batch to run in order. This supports duplicate calls to the same
parallel-safe tool without making side-effecting execution an implicit
optimization.

## Message contract

User and tool-result messages contain ordered text or image `ContentBlock`
values. Assistant messages contain ordered text, reasoning, or tool-call
`AssistantContent` values, a terminal `StopReason`, optional provider-reported
`TokenUsage`, and provider-continuity metadata. The separate flattened
`text + tool_calls` representation no longer exists, so a response such as
`text / tool call / text` reaches the next model request unchanged.

Provider adapters must retain opaque response IDs, response-model names, text
and reasoning signatures, tool thought signatures, and namespaces when their
provider requires those values on later requests. The SDK stores these values
but does not interpret them.

`ModelEvent::ContentDelta` and `AgentEvent::MessageUpdate` carry a content index
so consumers can keep simultaneous blocks distinct. `AssistantDelta` separates
visible text, reasoning, tool-call identity, and incremental JSON arguments.
Tool identity has an explicit start delta because an argument fragment cannot
identify its call; text and reasoning need no redundant start/end events.
`MessageStart` carries only the role because terminal outcome and usage do not
exist yet while a response is streaming. Streaming updates remain transient;
the completed ordered message is authoritative and is the only assistant
output written to `AgentState`.

`AgentRunResult::output` is a convenience string formed by concatenating the
final assistant response's visible text blocks in source order; reasoning is
excluded. Callers needing the full structure use Agent state or lifecycle
events.

Tool updates use a bounded channel and are emitted only as transient lifecycle
events. `ToolOutput` final content and optional JSON details become the one
model-visible `ToolResult`. A `Tool` must decode and validate raw JSON arguments
before performing effects; `ToolSpec::input_schema` is the schema advertised to
the model, not a second generic validator inside the Agent loop.

## Model outcome and accounting

`StopReason` has three successful provider outcomes: `Stop`, `ToolUse`, and
`Length`. Provider failures and cancellation remain typed Rust errors. Tool
calls in a `Length` response are converted to model-visible errors and never
execute because their arguments may be incomplete. Actual ordered tool-call
content, rather than a provider's reason label, decides whether another model
turn is needed; this tolerates imperfect compatibility adapters without
weakening the `Length` safety rule.

`TokenUsage` contains mutually exclusive input, output, cache-read, and
cache-write counts. An adapter must subtract cached input from ordinary input
when its provider reports an inclusive input total. Usage is optional because
unknown usage must not become a misleading zero. Every completed assistant
message keeps its own value. `AgentRunResult::usage` sums every model turn only
when all turns reported usage; otherwise it is `None`, while known per-message
values remain in `AgentState`. When a completed response is rejected by the
tool-call safety limit, its usage remains available on the typed error.

The SDK stores counts, not money. A host can combine them with the model used
for that run and a current or snapshotted price catalog to build token and cost
views. Model identity, pricing, currencies, budgets, and billing policy stay
outside the Agent SDK.

## Control and scheduling

`Agent::prompt()` starts with new user text. `Agent::resume()` samples the
existing transcript without duplicating its tail; this is the Rust equivalent
of Pi's `continue()` API. A user or tool-result tail can be retried directly. A
completed assistant tail requires queued steering or follow-up input.

An `AgentHandle` is clonable and remains usable while `Agent` is mutably
borrowed by `prompt()` or `resume()`. It provides cancellation, idle waiting,
and two FIFO input queues:

- steering enters at the next turn boundary, after every tool call from the
  current assistant message has produced a result;
- follow-up input enters only when there are no tool calls or steering messages
  left to process.

Steering always drains before follow-ups. Each queue can drain one message or
all available messages per boundary. Both queues share
`AgentConfig::max_queued_messages`; the default is 64. Scheduling returns a
typed error when that bound is full or the owning Agent has been dropped.
Only ordered user `ContentBlock` values can be queued, so a surface can send
text and images but cannot inject fabricated assistant or tool history through
the scheduling API. Text-only convenience methods use this same path.
`Agent::set_config` rejects a new limit below the number of messages already
accepted instead of silently discarding them or violating the advertised bound.

Queue contents are transient process state and are not part of `AgentState`.
A durable host that accepts remote scheduling must journal that command before
acknowledging it, then restore or redeliver it idempotently. That durability
belongs to the host or RCP adapter, not this embedded SDK.

These are deliberate safety differences from the studied Pi snapshot: Pi's
queues are unbounded and owned directly by its Agent object. Renoa uses a
bounded shared controller, closes orphaned handles, and claims assistant-tail
input before awaiting lifecycle listeners. This prevents memory growth,
invalid post-drop scheduling, and a clear-vs-resume race without adding network
or persistence concerns to the SDK.

The future returned by `prompt()` or `resume()` must be driven to completion.
An embedding should cancel through `AgentHandle::abort()` and keep polling the
run until it settles; dropping an arbitrary Rust future does not provide the
ordered shutdown guarantees of an Agent lifecycle.

## Policy and durability

The host chooses which `Tool` objects exist. A tool or its host adapter owns
authorization, approvals, sandboxing, and target restrictions. The Agent SDK
does not grant permissions, and RCP does not dictate them.

`AgentState` serialization is state transfer, not crash consistency. A durable
host must decide when state is committed and acknowledged. RCP's durable task
journal and `renoa-runtime` run ledger remain separate concerns.

Compaction is intentionally not an Agent SDK responsibility. A session host
owns complete append-only history, token-budget policy, summary generation, and
the compacted active transcript. `ContextProjector` can select that transcript
before each request while `AgentState` remains unchanged. The SDK does not
contain a compaction threshold, summarizer, or persistence mechanism.

`Agent::set_model`, `set_system_prompt`, and `set_tools` reconfigure subsequent
runs. Rust's mutable borrow prevents these mutations during an active
`prompt()` or `resume()`. Provider-specific model settings remain inside the
selected `Model` adapter.

## Deferred extension points

- custom host-only messages that need conversion before model requests;
- generic pre/post-tool policy hooks;
- graceful stop and mid-run model/tool replacement; and
- tool-result termination hints.

These are not required to embed the SDK or build the first Renoa harness. The
harness comparison must prove a real consumer and exact semantics before any of
them enter the core. Permission checks can already be enforced by wrapping a
`Tool`; provider conversion belongs in `Model`; compaction belongs in
`ContextProjector`.

## Deliberately outside the Agent core

- OpenAI, Anthropic, or other concrete provider adapters
- provider pricing, currencies, and cost calculation
- filesystem, shell, search, and other packaged tools

Those belong to adapter or host crates. They are required for products built on
the SDK, but are not Agent Core parity work.
