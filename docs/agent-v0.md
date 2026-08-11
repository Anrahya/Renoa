# Renoa Agent v0

## Purpose

Renoa Agent is a small, stateful Rust harness built around the existing kernel
loop. It can run inside a laptop process, on a VPS, or behind an RCP execution
node without importing surface or network code.

## Boundary

- `Engine` owns one bounded model-to-capability continuation loop and its
  durable per-command ledger.
- `Agent` owns one conversation across many calls to `prompt`.
- The host maps a task or local session to one `Agent`. It owns provider and
  capability configuration, authorization, scheduling, and durable state.
- An RCP execution-node adapter is one possible host. Telegram, Android,
  desktop, GitHub, and other surfaces never enter the Agent crate.

The Agent uses the same `ModelDriver` and `CapabilityHost` ports as the kernel.
An embedded caller uses the Rust API directly. A remote caller may add RPC or
RCP around that API without creating another agent loop.

## Proven behavior

The current tests prove that:

1. prompts from different surfaces can continue one conversation;
2. a host can serialize the conversation, rebuild the Agent, reopen its run
   store, and continue with the restored context;
3. restored state cannot replace the locally configured system instructions.

One `Agent` is exactly one conversation. A host must not share it between
tasks or principals. System instructions come from the local `ResolvedAgent`
definition and are rebuilt for every model request; they are not stored in the
portable conversation state.

`AgentState` is trusted host state. It is not an RCP payload and surfaces must
not be allowed to replace it.

## Durability limit

Serialization is a state-transfer format, not a crash-consistency guarantee.
The current run ledger and `AgentState` do not commit atomically. The RCP node
must not use the stateful Agent until it can serialize prompts for one task and
persist terminal run state plus the resulting conversation state as one durable
operation. Otherwise a crash could acknowledge work while losing its context,
or restore context for work that was never acknowledged.

## Next complete slice

Add durable session identity and atomic state persistence, then map each RCP
task to one serialized Agent execution lane. Streaming, context compaction,
provider adapters, and an RPC binding come later, only when a consumer needs
them.
