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
   prompt, context, and tool revisions remain frozen by the kernel. The fixed
   Host registry tools may read newly committed catalog state, but an exact
   catalog reference can never change underneath an invocation.
6. The first profile has all six local tools, `tool_search`, `tool_load`,
   `tool_execute`, `extension_manage`, `skill_search`, and `skill_load`.
   External schemas, installed-package metadata, and skill bodies are loaded
   into history only when Alpha requests them; their quantity never expands the
   model API tool list. Existing workspace boundaries and unrestricted Bash
   behavior remain unchanged.
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

## Agent Skills

Alpha does not receive a startup dump of every skill description or body. It
uses `skill_search` to query at most 200 names and short descriptions from the
Host's global and workspace `.agents/skills` sources plus installed Agent
Plugins, then passes one name to `skill_load`. Precedence is workspace, global,
then plugin. Different plugins cannot silently compete for the same skill name.
The loaded body, exact immutable revision, base directory,
compatibility note, and bounded supporting-file sample are available only after
that invocation.

Activation is durable session state owned by the Host. The activating command
receives the complete instructions in the tool result; later operations append
the same exact revision to Alpha's standing instructions. A crash retry of that
same command deliberately excludes its own new activation so its frozen runtime
does not change. Source edits are discoverable on the next search without
restart but cannot replace an already active revision. Active instructions
survive compaction and Host restart; historical full load results become short
model-facing receipts so the body is not duplicated. A skill supplies
instructions and files only. It cannot add a tool or permission.

## Extension management

Alpha receives one fixed `extension_manage` tool rather than one management
schema per package. It can search the replaceable discovery source, add one
exact refetched catalog candidate, one officially researched MCP definition,
or one inspected, digest-bound Agent Plugins 1.0 directory; inspect a local
package; install exact content; list installed revisions; and connect a
supported package MCP server to Alpha. It can also authorize or explicitly
restart a registered OAuth connection. Every add source becomes the same
immutable package. Supported skills hot-load first, then Renoa attempts the
connection. Discovery is a hint: the
Host revalidates catalog data and the real endpoint, while a miss directs Alpha
to official web research or an exact local package instead of guessing. Alpha
v1's deliberate full-access policy permits those actions; the tool does not
create a second approval system or expand the agent's authority.

Packages never contain credentials. A connection may name a Secret Service
bearer credential or select Host-owned OAuth. Named keys resolve only at the
request boundary. OAuth client state and tokens stay in Secret Service while
SQLite stores only a reference, non-secret recovery phase, and semantic
terminal receipt keyed by stable session/command/tool-call identity. Neither
kind of secret or remote failure text is returned to Alpha through that receipt
or stored in Renoa SQLite. Browser consent is
service authentication under Alpha's existing full-access scope, not a second
Renoa approval system. A successful connection
is visible to the next `tool_search` call without restarting Alpha or its
surface. A connection failure remains model-visible and preserves the installed
package and any successfully loaded skills. A package-provided skill is visible
to the next `skill_search` call without restart.

## Model-visible request

A normal Alpha request contains only:

1. the Alpha base prompt and applicable project instructions;
2. the exact Host-pinned active skill instructions;
3. the durable, context-projected conversation; and
4. the six local tool definitions, three fixed MCP-registry definitions, one
   fixed extension-manager definition, and two fixed skill-registry definitions
   in the model API's tool field.

Kernel command IDs, effect identities, recovery declarations, runtime
manifests, and configuration digests are not prompt content.

## Deliberate omissions

Alpha's profile contract does not define profile persistence, permission
vocabulary, subagents, MCP transport/catalog behavior, background jobs,
repository maps, automatic test policy, surface protocol behavior, or a
generic profile trait. The Host may resolve MCP-derived tools into Alpha
without making MCP part of Alpha. ACP can expose Alpha without becoming part of
Alpha. A later subagent capability may ask the Host to resolve another agent
with its own session, delegated authority, tools, model, and instructions; it
will reuse the same kernel and agent loop.

## Proof

The real headless product path must prove that:

1. Alpha's base prompt and root project instructions reach the model;
2. tool schemas and kernel bookkeeping are not duplicated in that prompt;
3. one durable session continues after changing model and reasoning level;
4. each operation freezes the exact selected model and reasoning revision;
5. the Alpha configuration digest remains stable across that selection change;
6. a 1,000-tool external catalog adds no model API schema, search returns only
   compact matches, and load returns only explicitly requested schemas;
7. exact external references fail stale instead of changing after refresh;
8. a skill added during a live session is discoverable without restart;
9. activated exact skill revisions survive compaction and Host restart without
   duplicating their full bodies in later model history; and
10. an Exa-shaped package is content-bound through inspect and install, sends
    its public header and a just-in-time bearer through the real MCP boundary,
    becomes searchable without restart, and never stores the key or adds an
    external schema to the normal model request; and
11. an OAuth package connection completes through the same `extension_manage`
    surface, survives callback cancellation and Host restart safely, refreshes
    once across concurrent sessions, replays a settled management call without
    a second browser or OAuth POST, and never exposes credential state to the
    model.
