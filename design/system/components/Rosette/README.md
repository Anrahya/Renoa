# Rosette

A rosette draws one agent as its tiles: the core, its six parts, and whatever it has added.

It is Renoa's agent glyph. Where a product would show a generic avatar or a status icon, Renoa can show what the agent is actually made of, at any size.

## Use it for
- 24-40px: next to an agent's name in lists, the navigation rail and mentions.
- 64-160px: agent headers and the overview of all agents, where plugin count and pending approvals matter.
- 280px and up with `labels`: the agent's Configure overview, where each tile links to that part's settings.

## You provide
`parts` (the parts that are set; unset seats render as empty sockets), `plugins` (a count or names), `proposed` (plugins waiting for approval), `size`, and `name` for the accessible label. `labels` and `versions` for the large size.

## Rules
- The layout is fixed: Model at the top, then clockwise Workflow, Context, Tools, Skills, Profile. Never reorder seats; people learn the positions.
- Plugins fill seats under Tools, in the order of `Renoa.GROWTH`, because they add to Tools.
- A proposed plugin shows the needs-you dot. Pair the rosette with a StateBadge that says Needs you.
- Keep the agent's portrait as its face. The rosette describes composition; it does not replace identity.
