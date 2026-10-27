# Tile

A tile is one hexagon on the Renoa board: the core, one of the six parts, a plugin, an app, or Control Center. RCP is not a tile; it is the line between two tiles.

The tile is the unit of the whole visual language. Everything that can be swapped is a tile, and every tile sits on a flat-top hex grid, so a page can show structure (what an agent is made of) as geometry instead of as a list.

## Use it for
- Showing what an agent is made of, or one part of it, on overview and configure pages.
- Diagrams of the system: the landing board, empty states, onboarding.

## Kinds
| kind | looks like | means |
| --- | --- | --- |
| `core` | solid ink | Renoa's kernel. Fixed. There is exactly one per agent, always at the centre. |
| `part` | part tint with its pigment edge | One of the six replaceable parts. Needs `part`. |
| `plugin` | card with the Tools edge | An ability the agent added to its tools. |
| `proposed` | dashed edge and a filled needs-you dot | A plugin the agent found and is waiting for you to approve. |
| `retiring` | muted, name struck through | A part or plugin on its way out. Show it only during the swap. |
| `surface` | card with an ink edge | An app connected over RCP: Discord, Telegram, GitHub. |
| `control` | card with a 2px ink edge | Control Center, joined to every agent. |

## You provide
`label` (one or two short words), optional `meta` (mono: a version, `plugin`, `approve?`), `size` (circumradius in px).

## Rules
- A hex always means structure; never use a hex for a status. Status is a circle (see StateBadge).
- Keep names to 12 characters so they fit at 52px and up. Below that, drop labels and use a Rosette.
- Pigments belong to parts. A plugin wears the Tools edge because it adds to Tools; no other tile borrows a part colour.
- Tiles move, they do not morph: a swap slides the old tile out along its axis and drops the new one into the same seat (see Motion in the brand book).
