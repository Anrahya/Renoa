# Prototype Instructions

Run the local server yourself and open the preview in the browser available to this environment. Do not give the user server-start instructions when you can run it.

Before making substantial visual changes, use the Product Design plugin's `get-context` skill when the visual source is unclear or no longer matches the current goal. When the user gives durable prototype-specific design feedback, preferences, or decisions, record them in `AGENTS.md`.

When implementing from a selected generated mock, treat that image as the source of truth for layout, component anatomy, density, spacing, color, typography, visible content, and hierarchy.

Build app UI in `src/`. Keep `.openai/hosting.json`, `worker/index.js`, `scripts/prepare-sites-build.mjs`, and `tests/sites-worker.test.mjs` intact so the same local prototype can be handed to Sites. Before a Sites handoff, run `npm run build` and `npm run test:sites`; the build must leave `dist/client/index.html`, `dist/server/index.js`, and `dist/.openai/hosting.json`.

## Renoa control-room decisions

- On 2026-09-28 the owner restarted the redesign from the public landing page: it must be very visually appealing and explain what Renoa is. Agent-page design is deferred. Control Center screens stay uncluttered at a glance; detail lives in nested, settings-style pages. The owner rejected the dark agent-card roster ("crew deck") home concept.

- On 2026-09-28 the owner said the current Control Room aesthetic and UI do not work and will be redesigned as a whole. Until that redesign, new UI (starting with Discord onboarding on the agent Configure page) is deliberately bare-minimum and functional: plain shadcn forms, no visual polish. Do not treat the present look as an approved baseline for the redesign.
