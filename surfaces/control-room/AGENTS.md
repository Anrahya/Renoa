# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## Renoa control-room decisions

- On 2026-09-07 the owner authorized a complete redesign into a visually impressive, informative Host control panel. `design/tasks-reference.png` and the existing decorative task console are historical references, not constraints on the redesign.
- The owner endorsed the revised warm, sculptural Host-room concept, saved as `design/host-rooms-reference.png`. Use this as the visual direction, superseding the initial table-and-inspector concepts. The intended result is radical and visually distinctive while remaining useful for real control.
- Rooms are interactive entry points: selecting one opens or expands its workspace and reveals the relevant controls. Preserve orientation with the Host context and an obvious return to the full map. Support keyboard activation, reduced motion, and a compact layout on smaller screens.
- Keep information progressively disclosed: the Host map shows room name, state, and actionable attention indicators; opening a room shows current work and primary controls; configuration, detailed logs, and historical evidence belong one level deeper. The owner explicitly requested avoiding clutter on 2026-09-08. Establish real GitHub review behavior before building out its full management UI.
- Distinguish agent controls (job instructions, tools, runs, schedules, and review policy) from connection controls (account, channels, bindings, and connection health). Opening a surface or connection does not transfer ownership of agents or credentials away from the Host.
- Organize the control panel around Host-owned agents, their work, automations, and shared connections. GitHub repository policy and review runs belong within that system; a Slack channel or GitHub repository must not appear to own an agent.
- Prioritize current work, actionable failures, exact reviewed commits, repository triggers, and meaningful controls over decorative illustrations or unsupported summary metrics. Generated design concepts may use clearly labeled example data; production screens must use real contracts and explicit empty, disconnected, and setup-required states.
- Show only data the current Host/RCP contract actually provides. Future Office, Library, and Settings views stay visibly unavailable instead of displaying invented agents, nodes, or capabilities.
- Compress secondary metadata into familiar Phosphor icons with accessible labels and tooltips. Keep task identity, state, errors, and actions in words.
- Task history, replay position, and pending commands are local durable state. Passkey connection tickets stay in memory only.
