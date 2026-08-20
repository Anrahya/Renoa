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
decoded durable transcript, each message's operation and journal sequence, the
active operation identity, the frozen request shape, and the latest activated
context checkpoint. It chooses either the next model-facing view, a compaction
plan, or an explicit capacity failure. The built-in `FullHistoryStrategy`
preserves projection-only behavior. `CompactingContextStrategy` adds Renoa's
bounded portable-summary policy using a host-supplied deterministic
`ContextSizer`. A strategy revision must change whenever its behavior changes;
an active operation accepts only the frozen revision. Context preparation never
mutates the semantic journal or performs external work.

Custom strategies construct their own typed `CompactionPlan` and the loop
validates its request shape and durable cut before persistence. The built-in
compactor also accepts a replaceable `ContextProjector`. It applies that
projector before every normal-request and retained-tail size estimate, so the
concrete normal request is sized after projection. Summary requests remain
isolated from this projection and advertise no tools. Projector behavior is
frozen through the surrounding context revision.

`CompactionPlanner` is the pure planning helper used by the replaceable
compacting strategy. Given
validated limits, an optional activated checkpoint, the exact normal request
shape, and model-aware sizing, it selects a safe durable prefix and constructs
the exact summary request. It keeps tool calls with their results, preserves the
active user request in the retained tail, bounds large tool output in summary
input, and uses monotonic binary searches instead of rebuilding every possible
prefix. It chooses the first safe cut meeting the retained-tail target, or the
largest dispatchable summary prefix when no cut can meet that target. It does
not call a model, write storage, or activate a summary. The loop executes its
plan as an ordinary persisted model effect and activates only a validated result
as a semantic event.

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

An activated portable summary is a separate semantic event with kind:

```text
renoa.agent.context-checkpoint.v1
```

Its payload contains the exact summary and the durable message sequence it
covers. Context checkpoints must be non-empty, refer to an earlier message, and
strictly advance the previous boundary. Unknown versions under this namespace
fail closed. They project history for the model; they never delete or rewrite
the full message journal.

Checkpoint schema 2 contains the loop program counter and in-flight compaction
intent:

```text
NeedModel(model_turns)
AwaitingModel(model_turns)
AwaitingCompaction(model_turns, exact_plan, max_attempts, attempt)
NeedTool(model_turns, calls, next_index)
AwaitingTool(model_turns, calls, next_index)
Terminal
```

Conversation history is reconstructed from durable semantic events. It is not
hidden in process memory or owned only by the checkpoint. The checkpoint keeps
the active tool batch and exact next index so restart never guesses which call
may run. While compacting, it also keeps the exact summary request, durable cut,
and bounded attempt counters so restart cannot silently re-plan work already in
flight.

Loop binding revision 7 adds durable summary execution, checkpoint activation,
and typed provider-overflow recovery. Revision 6 added durable message origins
to context input and the pure bounded compaction planner. Revision 5 added
replaceable, revision-frozen context projection. Revision 4 added loop-owned
durable cancellation closure. Revision 3 added explicit, honest closure of
unknown model and tool effects. Revision 2 added fail-closed tool-call identity
validation and typed live tool uncertainty.

## Execution rules

One admitted operation advances as follows:

```text
command
  -> commit user-message event
  -> prepare the durable transcript
       -> if oversized: persist exact summary request
          -> model effect
          -> validate and commit context-checkpoint event
          -> prepare again
       -> otherwise: project the model-facing view
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
- an externally implemented strategy can execute its own typed compaction plan,
  and an external projector is included before every relevant size decision;
- every summary request is an exact persisted model effect with no advertised
  tools, and only a non-empty response accepted by the strategy that created
  the plan may activate;
- the built-in `CompactingContextStrategy` additionally requires a complete,
  normally stopped, bounded response with all seven non-empty checkpoint
  sections;
- malformed summaries retry the same frozen plan only up to the strategy's
  explicit bound; cancellation, unknown outcomes, and a compactor provider
  rejection never invent or activate a checkpoint;
- process loss replays an unsettled safe summary effect with the same identity,
  while a settled summary activates after restart without another model call;
- a typed provider context-window rejection triggers the same compaction path
  without repeating the known-oversized normal request;
- durable cancellation balances every outstanding tool call in source order,
  while preserving a settled current result and distinguishing work that never
  dispatched from work that may have run; and
- a new operation reconstructs prior session messages from the event log.

Steering, follow-ups, approvals, parallel tool batches, transient streaming,
and authoritative settlement of an unknown effect are not implemented by this
runtime slice.

## Effect adapters and recovery

The model adapter decodes the exact persisted `ModelRequest`, invokes the
selected `Model` through `sample_model`, and returns one complete serialized
`ModelResponse`, a typed context-window rejection, a definite adapter failure,
or explicit uncertainty. A typed context-window rejection is settled durably so
the loop can request compaction; it is not copied into model-visible history.
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

Compaction uses the same model binding and recovery declaration as ordinary
sampling, but its exact persisted request advertises no tools. Before dispatch
and again before settlement, the loop validates that its saved boundary is
still a safe cut in the durable transcript. A valid response and its activated
checkpoint event commit in one kernel transition. An unknown summary outcome
blocks honestly; explicit abandonment or cancellation terminates without
creating a summary.

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

The original kernel handoff's generic foundation, model/tool-loop adaptation,
real local coding turn, and durable compaction migration are now complete. The
next migration remains consumer-gated: compose the existing kernel, loop,
provider, and tools into the first narrow product host—preferably the planned
read-only GitHub review agent—before adding another generic contract. ACP and
RCP remain separate later surface and continuity work.
