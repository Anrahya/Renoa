# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## Renoa control-room decisions

- `design/tasks-reference.png` is the selected visual direction, not a rigid pixel contract.
- Show only data the current Host/RCP contract actually provides. Future Office, Library, and Settings views stay visibly unavailable instead of displaying invented agents, nodes, or capabilities.
- Compress secondary metadata into familiar Phosphor icons with accessible labels and tooltips. Keep task identity, state, errors, and actions in words.
- Task history, replay position, and pending commands are local durable state. Passkey connection tickets stay in memory only.
