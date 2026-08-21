# Renoa Alpha v1

## Purpose

Alpha is Renoa's first local coding-agent profile. Its stable Host identity is
`renoa.coding.alpha.v1`.

Alpha is not a loop, model, session store, or surface. It is the Host-owned
recipe that supplies coding behavior and project instructions while the Host
selects the exact model, reasoning level, context strategy, workspace, and
tools. The kernel freezes the resolved behavior for each operation.

## Market study

The design was checked against primary source at these exact revisions on
2026-08-21. Renoa did not copy upstream source or prompt text.

| Project | Revision | License | Relevant evidence |
| --- | --- | --- | --- |
| [Pi](https://github.com/earendil-works/pi/tree/5cd93f688aaab89dbb6dfa4aca535f21796ae185) | `5cd93f688aaab89dbb6dfa4aca535f21796ae185` | MIT | Small generated prompt, selected tools, project context, and separately loaded skills |
| [Grok Build](https://github.com/xai-org/grok-build/tree/19d42e35c07a9c9244f03f6df0c4c353f970d4f9) | `19d42e35c07a9c9244f03f6df0c4c353f970d4f9` | Apache-2.0 | Agent definition resolved into a session-bound prompt, tool bridge, and policies |
| [OpenCode v2](https://github.com/anomalyco/opencode/tree/5e75e5e9901f0d178f425bfb47f1bd46cbe78a59) | `5e75e5e9901f0d178f425bfb47f1bd46cbe78a59` | MIT | One effective agent controls prompt and capability selection; plan/build are profiles rather than loop states |
| [Codex](https://github.com/openai/codex/tree/a3bce23f3b296e44d2d76c4fc2d6f105138aafd2) | `a3bce23f3b296e44d2d76c4fc2d6f105138aafd2` | Apache-2.0 | Bounded root-to-working-directory `AGENTS.md` discovery and explicit instruction provenance |
| [Gemini CLI](https://github.com/google-gemini/gemini-cli/tree/ba4296c6c9ee15ba849eb8cb76be8f8704365f3a) | `ba4296c6c9ee15ba849eb8cb76be8f8704365f3a` | Apache-2.0 | Stable operating instructions separated from project strategy and context |
| [OpenHands](https://github.com/OpenHands/OpenHands/tree/4a8cabc5fdc81bb6d899785f33ea7449387beb4c) | `4a8cabc5fdc81bb6d899785f33ea7449387beb4c` | MIT | Agent loop, execution runtime, conversation, and surface remain separate components |
| [Aider](https://github.com/Aider-AI/aider/tree/5dc9490bb35f9729ef2c95d00a19ccd30c26339c) | `5dc9490bb35f9729ef2c95d00a19ccd30c26339c` | Apache-2.0 | Repository maps and automatic verification can improve coding work without belonging in the base loop |
| [SWE-agent](https://github.com/SWE-agent/SWE-agent/tree/3ea751c087f32b16e039a2233dd6eefecef325d5) | `3ea751c087f32b16e039a2233dd6eefecef325d5` | MIT | Agent, environment, tools, history processing, and recorded trajectories have distinct responsibilities |
| [Goose](https://github.com/block/goose/tree/810bb68fff5d90c994a49af7a23e6abde0879970) | `810bb68fff5d90c994a49af7a23e6abde0879970` | Apache-2.0 | Installable extensions and reusable recipes belong above the core agent loop |

Cursor and Devin were reviewed only through their official product
documentation because their agent implementations are not open source. Their
observable session, rule, progress, and takeover behavior informed future
surface requirements, not Alpha's internal design.

## Decisions retained

1. Alpha has one small, versioned, provider-neutral base prompt.
2. Project rules stay separate from that base and retain visible provenance.
3. Tool definitions travel through the model API's tool field. Alpha does not
   repeat their schemas or runtime binding data in its prompt.
4. The Host may select a different model or reasoning level for the next
   operation without changing Alpha, its session, or its history.
5. An active operation never changes runtime. Its exact model, reasoning,
   prompt, context, and tool revisions remain frozen by the kernel.
6. The first profile has all six local tools. Existing workspace boundaries
   and unrestricted Bash behavior remain unchanged.
7. Alpha has no plan mode. A question, review, plan, or implementation request
   is handled according to the user's intent by the same agent.

## Project instructions

V1 loads the exact `AGENTS.md` at the canonical workspace root. The Host:

- accepts UTF-8 only;
- rejects a symlink that resolves outside the workspace;
- ignores a missing or empty file; and
- rejects content above 32 KiB instead of silently truncating instructions.

The Host captures this file while constructing Alpha's runtime configuration,
before asynchronous provider resolution. Its exact content contributes to the
kernel's frozen configuration digest.

The workspace root is also the current execution directory in this product
slice. Alpha is told to check for a nearer `AGENTS.md` before changing files in
a nested subtree. Host-driven root-to-current-directory discovery should be
added only when the Host gains a distinct current-directory consumer. Loading
every nested file eagerly would apply instructions outside their scope and
waste model context.

## Model-visible request

A normal Alpha request contains only:

1. the Alpha base prompt and applicable project instructions;
2. the durable, context-projected conversation; and
3. the currently selected tool definitions in the model API's tool field.

Kernel command IDs, effect identities, recovery declarations, runtime
manifests, and configuration digests are not prompt content.

## Deliberate omissions

Alpha's profile contract does not define profile persistence, permission
vocabulary, subagents, MCP, background jobs, repository maps, automatic test
policy, surface protocol behavior, or a generic profile trait. ACP can expose
Alpha without becoming part of Alpha. A later subagent capability may ask the
Host to resolve another agent with its own session, delegated authority, tools,
model, and instructions; it will reuse the same kernel and agent loop.

## Proof

The real headless product path must prove that:

1. Alpha's base prompt and root project instructions reach the model;
2. tool schemas and kernel bookkeeping are not duplicated in that prompt;
3. one durable session continues after changing model and reasoning level;
4. each operation freezes the exact selected model and reasoning revision; and
5. the Alpha configuration digest remains stable across that selection change.
