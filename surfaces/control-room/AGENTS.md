# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## Renoa control-room decisions

- On 2026-09-07 the owner authorized a complete redesign into a visually impressive, informative Host control panel. `design/tasks-reference.png` and the existing decorative task console are historical references, not constraints on the redesign.
- On 2026-09-09 the owner superseded the room metaphor with a quiet, flowing editorial interface: warm ivory, ink, restrained olive, readable typography, and generous whitespace. The room image is a historical reference, not the current layout target.
- Present work in a continuous readable flow, with completed activity compact and selected activity or evidence expanded inline. Avoid card grids, boxed sections, persistent inspector columns, and decorative network diagrams. Connecting lines must express recorded sequence or explicit dependencies, not imply causation between unrelated tasks or configuration fields.
- Keep information progressively disclosed: Host overview shows current work and actionable attention; selecting work reveals its runs and evidence; selecting an agent reveals its instructions, capabilities, automations, and delivery bindings. Keep Host context and an obvious return path. Support keyboard activation, reduced motion, and compact layouts.
- Distinguish agent controls (job instructions, tools, runs, schedules, and review policy) from connection controls (account, channels, bindings, and connection health). Opening a surface or connection does not transfer ownership of agents or credentials away from the Host.
- Organize the control panel around Host-owned agents, their work, automations, and shared connections. GitHub repository policy and review runs belong within that system; a Slack channel or GitHub repository must not appear to own an agent.
- Prioritize current work, actionable failures, exact reviewed commits, repository triggers, and meaningful controls over decorative illustrations or unsupported summary metrics. Generated design concepts may use clearly labeled example data; production screens must use real contracts and explicit empty, disconnected, and setup-required states.
- On 2026-09-08 the owner directed review execution failures and their causes to the Renoa control hub, not GitHub PR comments. Show the Host-owned incomplete outcome and diagnostics in the agent's run detail/attention view; GitHub publication stays suppressed for failed reviews.
- Show only data the current Host/RCP contract actually provides. Future Office, Library, and Settings views stay visibly unavailable instead of displaying invented agents, nodes, or capabilities.
- On 2026-09-09 the owner rejected the icon-heavy Transformers portrait experiment and stopped visual iteration. Keep names, states, errors, and actions in words; use familiar icons sparingly for utility controls. Character naming does not require large portraits or character-themed controls.
- Distinguish recorded activity, scheduled work, and explicitly agent-declared plans. Do not infer future steps, handoffs, shared context, progress percentages, or completion estimates from adjacent events. An agent may have multiple automations; pausing an automation is not pausing the agent. Show effective runtime capabilities separately from installed Host inventory and future recipe changes.
- Task history, replay position, and pending commands are local durable state. Passkey connection tickets stay in memory only.
