# Renoa agent contracts v0

## Purpose

`renoa-agent` is Renoa's provider-neutral model, message, tool, and live-event
vocabulary. It also supplies two leaf execution helpers: one model invocation
and one tool invocation.

It is deliberately not a second agent runtime. Conversation advancement,
durable admission, recovery, context policy, and tool ordering belong to the
kernel-backed loop in `renoa-agent-loop`. Every Renoa product agent uses the
non-replaceable kernel for execution truth.

The crate name is retained because these are the contracts from which an agent
runtime is assembled. It does not imply that the crate owns an `Agent` object.

## Boundary

The crate owns:

- ordered provider-neutral `Message` and content types;
- the `Model` port, request, response, stream, error, and token types;
- the `Tool` port, schema, call, output, error, and uncertainty types;
- transient `AgentEvent` observation types; and
- `sample_model` and `invoke_tool`, which execute exactly one external leaf
  call each.

It does not own:

- conversation or session state;
- a model-to-tool continuation loop;
- persistence, checkpoints, recovery, or command admission;
- context projection, compaction, memory, or token-budget policy;
- tool-batch scheduling, steering, follow-ups, or subagents;
- provider authentication, retries, catalogs, pricing, or wire formats; or
- tools, permissions, approvals, workspaces, surfaces, ACP, or RCP.

This split prevents an in-memory execution path from bypassing the kernel's
persist-before-effect and recovery rules.

## Message and model contract

User and tool-result messages contain ordered text or image `ContentBlock`
values. Assistant messages contain ordered visible text, reasoning, or tool
calls plus a terminal `StopReason`, optional normalized `TokenUsage`, and
provider-continuity metadata.

Provider adapters retain opaque response IDs, response-model names, text and
reasoning signatures, tool thought signatures, and namespaces when a later
request needs them. Renoa stores those values without interpreting provider
policy.

One `Model::stream` call is one logical model effect for an exact request. Its
stream may expose:

- the exact translated provider request;
- redacted response status and headers;
- indexed visible, reasoning, or tool-call deltas;
- adapter retry diagnostics; and
- one completed response or one typed failure.

`sample_model` assigns one correlation identity, awaits observations in order,
and accepts only a completed response as settled output. A stream that ends
without completion is uncertain. Cancellation before provider dispatch is
known not to have started; cancellation or failure after provider traffic or
assistant output begins is outcome-unknown unless the adapter proves a terminal
result first.

Adapters may perform bounded pre-output retries under their documented policy,
but every attempt is observable through retry events and belongs to the same
durable effect. Retrying after assistant output starts is forbidden.

`TokenUsage` has separate uncached input, output, cache-read, and cache-write
lanes. `None` means the provider did not report enough information; unknown
usage must not become zero. Pricing and budgets remain Host policy.

## Tool contract

A `Tool` exposes one model-visible `ToolSpec` and executes one complete
`ToolCall`. It must decode and validate raw arguments before effects. The JSON
schema is what the model sees, not a second generic validator inside Renoa.

The tool future must observe cancellation and resolve only after work it owns
has stopped. A process tool must kill and reap its process group. Detached work
violates the contract.

`invoke_tool` has three outcomes:

1. successful output becomes one `ToolResult`;
2. a definite failure becomes a model-visible error result with a stable code
   and partial-change flag; or
3. an effect whose final outcome cannot be proven remains a typed
   `ToolOutcomeUnknown` and must not be rewritten as an ordinary failure.

Progress uses a bounded channel, is awaited in order, and stays transient.
The durable `renoa-agent-loop` adapter owns tool start, terminal success, and
unknown-outcome observations around `invoke_tool`. Tool batches are currently
executed sequentially in model source order. No unused parallel-safety field is
part of the tool contract.

Empty or duplicate call identifiers are rejected by
`validate_tool_call_ids` before a durable loop dispatches any call.

## Observation contract

`AgentEventSink` is a Host-owned observer for live presentation and diagnostic
tracing. Emission is awaited to preserve order and backpressure. Events are not
authoritative history and may stop without a terminal event if execution is
lost.

The kernel journal remains the source of truth. A surface reconnects from that
durable history; it never reconstructs truth from transient deltas.

## Proven behavior

The crate's focused tests prove:

- pre-dispatch cancellation does not invoke a model or tool;
- post-dispatch model cancellation and incomplete streams remain uncertain;
- a completed model response wins over later cancellation drain;
- provider diagnostics preserve one correlation identity and typed failure
  evidence;
- definite, unavailable, and uncertain tool outcomes stay distinct;
- bounded tool progress is delivered in order; and
- tool-call identifiers must be non-empty and unique.

The real kernel path additionally proves complete model/tool/model turns,
source-ordered sequential tools, cancellation, safe replay, unsafe-effect
blocking, context compaction, and restart recovery. Those tests live in
`renoa-agent-loop`, `renoa-local`, and `renoa-acp` because that is where the
durability and product boundaries actually exist.

## Retired path

The former standalone in-memory `Agent`, portable `AgentState`, control queues,
context projector, and parallel scheduler were removed when `main` was
consolidated around the kernel-backed foundation. No current product path used
them, and retaining them would create a second, weaker execution architecture.
Their implementation and historical tests remain recoverable through Git
history; they are not compatibility commitments.
