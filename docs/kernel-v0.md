# Renoa kernel contract v0

> This kernel is one reference executor for RCP. It is not Renoa's continuity
> protocol and is not required by an RCP coordinator or surface. Pi SDK or
> another harness may replace it behind an execution-node adapter.

## Purpose

The kernel executes an agent turn independently of the surface that submitted
it and independently of the model provider that performs inference. A surface
transports commands and events; it does not own the agent loop.

## Walking-skeleton scenario

The first acceptance scenario asks a scripted model to read a file, edit it,
run a verification command, and return a final answer. The run must remain
inspectable after the process releases and reopens its SQLite store.

## Invariants

1. Every input is represented by a typed command envelope carrying command,
   principal, surface, and target identity.
2. Command admission is atomic by command identity. An exact retry references
   the original run; changed command content or changed local runtime
   configuration under the same identity is rejected.
3. The execution-node adapter supplies locally resolved instructions and
   capability grants for each run. The kernel freezes and persists that
   snapshot before inference. RCP does not carry it.
4. The model receives the resolved instructions as its system message.
5. A run is durable before the first model invocation begins.
6. The model cannot perform side effects directly. It may only request named
   capabilities advertised by the capability host.
7. Only capabilities present in both the resolved grant and the host manifest
   are advertised. An unadvertised call is returned to the model as an error
   and never reaches the host.
8. Every model invocation and capability request produces a durable run event.
   Each capability completion is persisted before the next call executes.
9. Each model response has a configured capability-call limit, defaulting to
   64. Accepted calls execute in source order. Parallel execution may be added
   only when a capability explicitly guarantees that it is safe.
10. Capability output has a bounded model-facing value and an explicit error
   flag. Additional channels are added only when the runtime consumes them.
11. Model context is the ordered turn history in v0. A context-policy port is
   introduced only when a second real projection strategy exists.
12. A run transitions from open to exactly one terminal state: completed,
   failed, or cancelled.
13. Storage, model drivers, and capability hosts are ports.
14. The kernel contains no surface-specific or domain-specific policy.

## Durability boundary

The v0 store is a durable audit ledger, not a crash-resumption engine. A
process failure can leave an open run whose last requested side effect has an
unknown outcome. Renoa must not automatically replay that operation until a
future capability contract defines idempotency or reconciliation semantics.

## Explicit non-goals for v0

- Real provider APIs or provider authentication
- Agent discovery, configuration parsing, or authorization policy resolution
- Streaming model output
- Conversation memory and compaction
- Human approval workflows
- Sandboxed or remote execution
- Steering and queued follow-up messages
- Automatic recovery or replay of interrupted capability execution
- Telegram, GitHub, Jira, Android, or desktop adapters

The filesystem and command capability host exists only inside the integration
test. Renoa will not expose local execution until it has a real sandbox and
approval contract.
