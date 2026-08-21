# Renoa

Renoa is a modular agent system built around one small, non-replaceable durable
kernel. The kernel preserves execution truth: stable identities, command
admission, ordered state, effects, cancellation, recovery, semantic history,
and frozen runtime revisions.

Everything that defines an agent's behavior stays replaceable outside that
kernel: model provider, agent loop, context and compaction strategy, tools,
skills, workspace policy, permissions, and surfaces.

```text
surface -> Renoa Host -> Renoa Kernel -> resolved loop and effect adapters
                |
                `-> profile + model + context + tools + workspace policy
```

The local Host assembles a concrete Agent instance from those pieces for each
new operation. The kernel freezes that exact assembly, then executes it
durably. This is the base for adding different agents and future capabilities
without turning the kernel into product policy or one giant plugin interface.

## Current local coding path

- [`renoa-kernel`](crates/renoa-kernel) is the non-replaceable durability core.
- [`renoa-agent`](crates/renoa-agent) defines provider-neutral messages, models,
  tools, and streaming events.
- [`renoa-agent-loop`](crates/renoa-agent-loop) is the replaceable model/tool
  loop with durable, replaceable context compaction.
- [`renoa-local`](crates/renoa-local/README.md) is the first Host. It composes
  Renoa Alpha, Pi provider routing, the compaction strategy, and six local
  coding tools.
- [`renoa-acp`](crates/renoa-acp) is a thin ACP v1 surface adapter. A compatible
  frontend launches `renoa-agent acp`; ACP translates UI messages and live
  updates but does not own agent composition or durability.
- [`ui`](ui/README.md) is the first Renoa desktop surface. Its Tauri process
  owns only the local ACP child and transport; the React UI uses the standard
  ACP client contract.

Alpha currently has `read_file`, `edit_file`, `write_file`, `bash`, `grep`, and
`find`. Model and reasoning choices can change between operations. Workspace
`AGENTS.md` instructions are read again for every new operation, while an
already admitted operation keeps its frozen behavior.

After the one-time Fedora packages and SuperGrok login documented in the
[desktop README](ui/README.md), launch the complete local path with:

```sh
pnpm --dir ui desktop:dev:local
```

## RCP is separate

RCP is Renoa's future continuity layer for authenticated communication and
handoff across surfaces and execution nodes. It is intentionally not part of
the current local Agent path and does not replace, wrap, or define the kernel.

This repository still contains the earlier loopback RCP coordinator, clients,
node bridges, and deployment proof. Their canonical continuity decisions live
in [`docs/rcp-v0.md`](docs/rcp-v0.md). They remain isolated while the first
local surface and Host contract become real.

## License

Renoa is available under either the
[Apache License, Version 2.0](LICENSE-APACHE) or the [MIT license](LICENSE-MIT),
at your option.

Unless explicitly stated otherwise, contributions intentionally submitted for
inclusion in Renoa are provided under those same terms without additional
conditions.

Start with the current
[`renoa-kernel` contract](docs/renoa-kernel-v0.md),
[`Host architecture`](docs/renoa-host-v0.md),
[`Alpha profile`](docs/renoa-alpha-v1.md),
[`agent-loop contract`](docs/renoa-agent-loop-v0.md), and
[`ACP adapter`](docs/acp-v1.md).

For continuity work, [`docs/rcp-v0.md`](docs/rcp-v0.md) remains canonical.
[`docs/harness-v0.md`](docs/harness-v0.md) and
[`docs/kernel-v0.md`](docs/kernel-v0.md) are archived records of superseded
implementations, not the current Renoa architecture.
