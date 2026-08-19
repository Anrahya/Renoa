# Renoa model/tool loop v0

## Status and authority

This document defines the first real runtime plugin above `renoa-kernel`.
[`renoa-kernel-v0.md`](renoa-kernel-v0.md) remains authoritative for durable
execution laws. [`agent-v0.md`](agent-v0.md) remains authoritative for the
provider-neutral model, message, and tool vocabulary. This runtime consumes
both without making either depend on it.

The implementation lives in `crates/renoa-agent-loop`. It is a replaceable
agent behavior, not part of the non-replaceable kernel. A different loop can
implement `LoopPlugin`, define its own checkpoint and events, and use different
effect adapters without changing kernel control flow or schema.

The current capability status and deliberately deferred gaps are tracked in
[`agent-loop-readiness.md`](agent-loop-readiness.md).

Future model, tool, and loop capabilities should reuse established external
standards at adapter boundaries where their semantics fit. A Renoa-specific
contract is justified only by a concrete durability or execution invariant the
external contract does not express. The exact standard, version, and mapping
must be chosen and tested with the consumer that needs each capability; this v0
does not reserve speculative fields for them.

## Dependency direction

```text
renoa-agent             renoa-kernel
messages/model/tools    durable execution laws
          \             /
           renoa-agent-loop
           model/tool behavior
                   |
              renoa-local
          provider/workspace host
```

`renoa-kernel` does not depend on `renoa-agent`, a provider, a workspace, or
this loop. `renoa-agent-loop` does not access SQLite, files, processes,
credentials, or a network except through kernel-dispatched effect adapters.

## Runtime composition

`build_runtime` consumes four concrete host choices:

- `AgentLoopConfig`: system instructions and non-zero model/tool-call limits;
- `ContextBinding`: a pure context strategy and its stable revision;
- `ModelBinding`: any `renoa-agent::Model`, its stable revision, and recovery
  declaration; and
- zero or more `AgentToolBinding` values: any `renoa-agent::Tool`, a stable
  revision, and recovery declaration.

The builder validates context, model, and tool revisions, computes a content
digest over the context revision, instructions, limits, advertised tool order
and specifications, and recovery declarations, and then creates the exact
kernel `RuntimeManifest`. Provider, context-strategy, and tool replacement
therefore change bindings, not kernel code. Replacing the loop means supplying
another `LoopPlugin` to the kernel.

`ContextStrategy` is synchronous, pure loop policy. It receives the complete
decoded durable transcript, each message's operation and journal sequence, and
the active operation identity. It returns only the ordered messages visible to
the next model request. The built-in `FullHistoryStrategy` preserves current
behavior. A strategy revision must change whenever its behavior changes; an
active operation accepts only the frozen revision. Projection never mutates the
semantic journal.

`CompactionPlanner` is a pure helper for replaceable context strategies. Given
validated limits, an optional activated checkpoint, the exact normal request
shape, and model-aware sizing, it selects a safe durable prefix and constructs
the exact summary request. It keeps tool calls with their results, preserves the
active user request in the retained tail, bounds large tool output in summary
input, and uses monotonic binary searches instead of rebuilding every possible
prefix. It chooses the first safe cut meeting the retained-tail target, or the
largest dispatchable summary prefix when no cut can meet that target. It does
not call a model, persist a checkpoint, or activate a summary. Those stateful
steps must be mediated by kernel effects in the next slice.

## Durable formats

The loop accepts one versioned JSON command shape:

```text
AgentCommand { content: Vec<ContentBlock> }
```

Each model-visible message is a semantic event with kind:

```text
renoa.agent.message.v1
```

The payload is exactly one provider-neutral `renoa-agent::Message`. Other
semantic event kinds are ignored because they may belong to observers or other
runtime features. An unknown version under the `renoa.agent.message.` namespace
fails closed instead of silently changing model history.

Checkpoint schema 1 contains only the loop program counter:

```text
NeedModel(model_turns)
AwaitingModel(model_turns)
NeedTool(model_turns, calls, next_index)
AwaitingTool(model_turns, calls, next_index)
Terminal
```

Conversation history is reconstructed from durable semantic events. It is not
hidden in process memory or owned only by the checkpoint. The checkpoint keeps
the active tool batch and exact next index so restart never guesses which call
may run.

Loop binding revision 6 adds durable message origins to context input and the
pure bounded compaction planner. Revision 5 added replaceable, revision-frozen
context projection. Revision 4 added loop-owned durable cancellation closure.
Revision 3 added explicit, honest closure of unknown model and tool effects.
Revision 2 added fail-closed tool-call identity validation and typed live tool
uncertainty. The checkpoint schema remains 1 because the program-counter
representation did not change.

## Execution rules

One admitted operation advances as follows:

```text
command
  -> commit user-message event
  -> project the durable transcript into a model-facing view
  -> persist exact model request
  -> model effect
  -> commit complete assistant message
  -> zero or more persisted sequential tool effects and tool-result events
  -> next model effect
  -> commit final assistant message and complete
```

Before accepting a settled effect, the loop checks that its binding and exact
request agree with the durable checkpoint and reconstructed transcript. A tool
result must retain the call identity and name from its request.

The implemented behavior matches the shared Renoa loop rules needed by this
slice:

- only complete model responses become semantic history;
- tool calls run sequentially in source order;
- another model request occurs only after every assistant tool call has one
  ordered, authoritative result;
- an unknown tool becomes a model-visible error without starting an external
  effect;
- calls from a length-stopped response are not executed and receive
  model-visible errors;
- empty or duplicate model tool-call identifiers fail the operation before the
  assistant message or any tool effect is committed;
- model-turn and per-response tool-call limits fail the operation explicitly;
- a context strategy can replace the model-facing message view without
  deleting or rewriting durable history;
- a context strategy can derive a deterministic, bounded compaction plan from
  real durable operation and sequence metadata without changing that history;
- durable cancellation balances every outstanding tool call in source order,
  while preserving a settled current result and distinguishing work that never
  dispatched from work that may have run; and
- a new operation reconstructs prior session messages from the event log.

Durable summary execution and activation, steering, follow-ups, approvals,
parallel tool batches, transient streaming, and authoritative settlement of an
unknown effect are not implemented by this runtime slice.

## Effect adapters and recovery

The model adapter decodes the exact persisted `ModelRequest`, invokes the
selected `Model` through `sample_model`, and returns one complete serialized
`ModelResponse`, a definite pre-inference failure, or explicit uncertainty.
An incomplete stream, cancellation after dispatch, or provider error whose
kind is `OutcomeUnknown` blocks the durable operation instead of becoming a
false terminal failure. Provider wire formats, authentication, and
provider-internal transport behavior remain inside the selected model.

Each tool adapter decodes one exact persisted `ToolCall` and invokes its
selected `Tool` through `invoke_tool`. Success and definite failure return one
complete serialized `ToolResult`. If the tool cannot prove whether its external
action completed, the adapter returns `EffectCompletion::OutcomeUnknown`; it
never serializes uncertainty as a failed result. The kernel's attempt-scoped
cancellation signal is passed through unchanged, and the tool must stop work it
owns before resolving. Workspace policy and side-effect authorization remain in
the host tool.

The host selects `SafeToReplay` or `NeverReplay` per binding. The kernel, not
the adapter or loop, applies that declaration after process loss. Tests prove
that an interrupted safe model invocation reuses the same effect identity and
request, while an interrupted never-replay tool becomes `OutcomeUnknown` and
is not called again. A dropped drive also holds session ownership until its
in-flight adapter has completed cancellation cleanup.

The host may explicitly abandon an `OutcomeUnknown` operation. For an unknown
model effect, the loop records no invented assistant response. For an unknown
tool effect, it emits one error result saying the current call may have
finished, followed in source order by error results saying later sequential
calls were not run. This closes the provider-neutral transcript without
claiming that the uncertain external action failed or never happened. The
kernel commits those events with the failed operation, releases queued work,
and leaves the effect itself permanently unknown. A repeated abandonment adds
nothing and returns the same terminal outcome.

The host may instead durably cancel any active operation. Cancellation of model
work records no invented assistant response. Cancellation before tool dispatch
records a model-visible error saying the call was not run. Cancellation after a
definite tool result preserves that exact result; cancellation after uncertain
dispatch says the call may have finished. Every later sequential call receives
a call-matched not-run error. The operation ends without making another model
request, but the balanced history is visible to the next operation's model
request. Internal adapter errors are never copied into these tool messages.

## Local proof

`renoa-local` exposes its existing guarded read, edit, write, and Bash tools as
`AgentToolBinding` values in addition to the legacy harness bindings. The
kernel coding-turn test uses a deterministic scripted model and the real local
edit tool to change a workspace file through this path:

```text
Kernel -> agent loop -> model adapter -> tool adapter -> LocalWorkspace
```

The model boundary is deterministic; the file mutation, path confinement,
effect persistence, transcript construction, and terminal operation path are
real.

## Relationship to the durable harness

The loop rules and leaf calls are adapted from Renoa's own `renoa-agent` and
`renoa-harness` behavior. No external source is copied. The pure planner and
bounded checkpoint formatter were adapted from repository commits
`6e8fccdb193f801b21812c14364752aaa30621c5` and
`47eddbc5de74113fcb688f3f739b943e6e96826e`, under this repository's
`Apache-2.0 OR MIT` license. The older harness remains intact while this
consumer proves the smaller kernel boundary. Its unknown-tool abandonment
behavior has been replaced on the kernel path by the generic kernel transition
and loop-owned transcript closure.

The next migration slice remains consumer-gated: execute the planned summary
as a persisted kernel effect, activate its checkpoint durably, and prove retry
and restart behavior without moving context policy into the kernel.
