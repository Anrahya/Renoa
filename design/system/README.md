Renoa is an open-source system for running AI agents on your own server. Its message is two things: every agent shares **one library** of plugins and skills, and every app and agent talks over **one line**, RCP. When an agent needs an ability it doesn't have, it finds one and adds it to the library, and every other agent can use it. Apps and agents connect over RCP, so no single app owns a task, and agents ask each other for help over the same line. Underneath, each agent is one small core that keeps the record, with six parts around it that can each be replaced. Control Center is where the owner runs all of it.

The visual system draws exactly that: **tiles on a board**. The core is a solid ink tile. The parts are six coloured tiles around it. Plugins are tiles that gather under Tools. RCP is the line between two things, drawn along the seams between tiles. Control Center is the tile joined to every agent. Every page, from the landing page to a settings screen, uses this one picture.

## The idea in four rules

1. **Hex means structure, circle means live state.** Draw the core, parts, plugins, apps and Control Center as hexagons (Tile, Rosette, PartChip), and RCP as the lines between them. Draw status only as a round dot or pill (StateBadge). Never cross them.
2. **Colour means a part.** Each of the six parts owns one pigment (`part-model` … `part-profile`) everywhere it appears. The one thing that borrows a pigment is what adds to a part: a plugin wears the Tools edge, because it adds to Tools. A pigment never sets text, but it may mark words that point at a part: their dotted underline, the underline of the step being shown and the tether from it.
3. **Ink is the accent.** Put `foreground` on `background` for headings, actions and the words that lead; running text is `foreground-soft`. `primary` is ink, so the one main action on a view is a solid ink button. There is no brand hue.
4. **Show change as movement, not decoration.** When something is swapped, slide the old tile out along its axis and drop the new one into the same seat. Everything else stays still.

## Content fundamentals

Write plainly, the way a knowledgeable friend would explain it. Use short sentences in active voice.

- Speak to the owner as **you**. Refer to agents by name, and as **it**: "Agent A found a plugin. It needs you to approve the connection."
- Use sentence case everywhere: titles, buttons, tabs and labels. Uppercase appears only in mono eyebrows (`type-eyebrow`), which are set in capitals by the style.
- Name things by what the owner recognises. Use the six part names exactly: Model, Workflow, Context, Tools, Skills, Profile. The fixed centre is the **core**. Added abilities are **plugins**, and they join an agent's **tools**. What assembles agents and keeps the shared library is the **host**. Discord, Telegram and GitHub are **apps**. **Control Center** is where the owner runs the system: it watches every agent, talks to it and runs its schedules. RCP is **RCP**; on first mention say "Renoa's own protocol".
- Buttons say exactly what happens ("Swap model", "Approve Figma", "Remove plugin"). Report the result in the same words ("Model swapped").
- Errors say what went wrong and how to fix it, without apology: "Agent A can't see this channel. Invite it to the server, then try again."
- Never claim what the system does not record. A schedule is not a heartbeat; an illustration is labelled "Illustration".
- Renoa is a **system**: the kernel, the host, RCP and Control Center. Sell the system, not an assistant. Its agents are agents, never a crew, team or bots. In public material, name example agents plainly (Agent A, Agent B) and say what each does; the owner's own agent names belong in their own Control Center. Describe change with **extend**, **add**, **swap** and **replace**. Never use evolve, grow, organism or other biology words.
- Tie change to tasks: parts change between tasks, never in the middle of one. Say so wherever a swap is offered.
- No emoji and no exclamation marks.

Real copy to match:

> Your agents share one library and talk over one line.
>
> Add it once. Every agent can use it.
>
> The same line runs between agents.
>
> Connections are temporary. The task is not.

## Visual foundations

### Colour
- Use `background` (paper) as the ground. Use `card` for sheets and inputs that sit on it, and `popover` for floating layers.
- Use `foreground` for headings, labels and the item being shown, and `foreground-soft` for running text such as paragraphs and ledes, so the page is not a wall of ink. Use `muted-foreground` for captions, metadata and inactive navigation. A headline may set its connecting words in `muted-foreground` so its promises, in ink, lead. Both hold 4.5:1 or better on `background`, `card` and `muted` in both themes.
- Draw dividers with `border` hairlines. Give controls an `input` edge, which holds 3:1 or better.
- The part pigments are fixed. Model is `part-model`, Workflow `part-loop`, Context `part-context`, Tools `part-tools`, Skills `part-skills` and Profile `part-profile`. Each has a `-tint` for tile fills and hovered rows.
- The state scale has one owner. `state-needs-you` is ink because it is the state the owner acts on. `state-failed` is only for work that broke. `state-running` covers work in progress. `state-idle` means nothing is pending. Every state also carries its word.
- Charts reuse pigments (`chart-1` … `chart-5`), so always give them a legend with words.
- Two themes ship: **Paper** (light, the default and the landing page) and **Graphite** (dark, for Control Center at night). Every token is defined in both.

### Type
- Set everything in Instrument Sans (`--font-sans`). Use IBM Plex Mono (`--font-mono`) only for eyebrows, figure captions, tile metadata and stored identifiers.
- Landing pages: use `type-display` once for the hero and `type-headline` for sections. Add `rn-condensed` to both (width 86).
- Control Center: `type-title` for page titles, `type-heading` for sections, `type-body` for text, `type-label` for controls and `type-caption` for metadata.
- Identifiers, versions and preset names use `type-code`, never a heading style. For example: `renoa.personal.assistant.v3`.
- Keep running text near 65 characters wide. Give headings `text-wrap: balance`.

### Space, shape and layout
- Space on a 4px step (`space-1` … `space-24`). Keep the page gutter at `space-4` or more.
- Radii are small and square-ish. Use `radius-md` for buttons and inputs, `radius-lg` for plates and dialogs, and `radius-xs` for chips and the switch knob. `radius-pill` belongs to state badges only.
- Lay records out as ruled rows (RecordRow), not card grids. The only bordered container is the Plate, and it frames figures.
- The board lattice (`board-line`, the `rn-board` class) sits behind figures, empty states and heroes. It shows empty sockets, meaning room to extend. Keep it faint; it never competes with tiles.
- Tile geometry: flat-top hexagons on axial coordinates. `tile-radius` is 60px at full size, `tile-grout` 5px between tiles, `tile-corner` 3px and `tile-stroke` 1.25px (non-scaling). The seats around a core are fixed: Model at the top, then clockwise Workflow, Context, Tools, Skills and Profile. Plugins fill `Renoa.GROWTH` in order, gathered under Tools.

### Depth
Keep the system flat. Separate things with hairlines and tints, not shadows. `shadow-pop` is only for floating layers.

Tiles are the one exception, because they are pieces on a board. Each tile has a short body (`tile-depth`, 4px) that shows as a band along its lower edges. A tile lifts (`tile-lift`, 18px per unit) while it is swapped, carried to another agent or waiting for approval, and its shadow shows on the socket below. A proposed tile has no body yet: it floats, dashed, over the seat it wants. An opened part is the tallest thing on the board: its face rises about 1.3 lifts and its walls reach down to its socket, the front wall in its pigment, the left lighter and the right darker.

### Motion
Motion exists to show change.

- **Swap:** the old tile lifts, then slides out along its axis about 0.7 tile and fades to `retiring`. The new tile arrives lifted above the same seat and drops into it. About 1.3s in all, ease `cubic-bezier(.2,.8,.2,1)`.
- **Wait:** a part that becomes ready during a task waits dashed beside its seat, and takes the seat only when the task ends.
- **Extend:** a `proposed` tile appears dashed in the next free seat at the edge. It shows the needs-you dot while it waits for approval, then fills and settles, and the empty sockets around it flash once in widening rings.
- **Open:** selecting a part raises it into a block. What it holds slides out from under it, 80ms apart, to sockets two steps away with one empty socket between them, and a line in the part's pigment draws along the seams to each. Everything else in the agent sinks to 16% and the core to 50%. Model, Workflow and Profile use one thing at a time: the one in use is tinted, and choosing another sends a packet from it to the block, which takes it over. Context, Tools and Skills hold several: the block sends a packet to the one you choose. Every fan ends in a dashed tile (`+`, "Any model", "Any app") in the outermost place, and the empty sockets past it flash once, to say there is room for as many more as you add. Selecting the block, the core or Escape closes it, and everything slides back.
- **Team:** an ink-edged tile beside an agent's Workflow and Context opens the same way onto other agents, drawn as small dark cores. Each ask goes out and comes back before the next.
- **Shared library:** a plugin or skill is drawn once, in the seat of the agent that added it. An agent that turns it on gets a tile with the same icon beside its cluster, joined back to the original by a line in the part's pigment, and a packet runs the line as it is turned on. There is never a second original: the line, not a duplicate, says it is shared. What agents learn about you is shared the same way, as lines between their Profile tiles.
- **Assemble:** a page that introduces an agent drops its tiles into their sockets: core first, then the parts clockwise, then what it has added, 90ms apart.
- **RCP:** RCP is the line itself. A line runs along the seams from an app, Control Center or another agent to the edge of an agent's cluster. Packets travel it with a short trail, at the same on-screen speed at every zoom, and the tile they reach pulses once. A closed app's line turns dashed; the agent and its task stay.
- **Agent to agent:** one agent asks another over RCP, and the answer comes back to the app you are in. The ask travels the line between the two agents, and the other agent can reach its own app on the way.
- **Control Center:** status comes in from every agent along its line. When an agent stops, its core shows the failed dot and its parts go dim until it is brought back.
- **Narrate:** one Note at a time, beside the tile that is acting. It moves when the action moves.
- **UI:** hovers and toggles take 160–220ms, easing out.
- Honour `prefers-reduced-motion`: stop ambient motion, keep the final states and offer Play. Anything that moves on its own gets a Pause control.

### Diagrams and text
A figure and the words next to it point at each other.
- A dotted underline (`rn-cue`) marks words that are on the board. Hovering or focusing them draws a tether from the end of their line to their tile, ending in a hex outline. Hovering a tile finds its words.
- The words being shown right now carry a highlight, and the tether follows them. Selecting them plays that moment again.
- Scroll, not the pointer, drives the story. A section with several marked items steps through them as you scroll: on a wide screen its text pins in place and each item gets about half a screen of scrolling; on a phone an item takes over when it crosses the reading line. The item being shown is ink and underlined in the pigment of the part it concerns (ink when it concerns no part), the rest wait in `muted-foreground`, and a row of small hexes beside the kicker shows which step you are on. An item starts the moment it is reached, even mid-scroll: its note, its tether and its first movement appear at once, never after the page stops. The figure repeats that item's moment until you move on, and an opened part stays open while its row is the one being shown. After a resize, read the scroll position again, because stepping sections change height.
- Hover and selection are extras on top: hovering still draws the tether, and selecting an item plays it straight away.
- Scroll chooses the scene; a scene never depends on scroll position to finish. A section changes only after the reading line has rested in it for about 160ms, and going back one section takes a clear move up. Nothing new starts while the page is still scrolling.
- Leaving a section settles whatever was mid-way by fading it out, never by cutting it. The camera follows a critically damped spring, so a new target never makes it lurch.

### Focus and states
- Focus is a 2px solid `ring` with a 2px offset on every interactive element.
- Hover washes with `accent`. Disabled drops to 45% opacity and loses pointer events.

## Iconography
- Use Phosphor icons (`@phosphor-icons/react`, already in Control Center): regular weight, 16px in controls and 20px in navigation, coloured with `currentColor`.
- The hex glyphs (PartChip, Kicker) are not icons. They mark parts and sequence.
- Apps and services on the board (Discord, GitHub, Telegram, Gmail, Claude and so on) carry their brand mark from Simple Icons (CC0), in ink, never in brand colour. A tile with no name shows its mark large; a named tile shows it small above the name. A dashed tile's mark is `input` grey.
- Never use emoji.

## Logo
The mark is a rosette: the ink core with six outlined tiles, one of them pulled out as if mid-swap. Use `renoa-mark-ink.svg` on Paper and `renoa-mark-paper.svg` on Graphite. Set the wordmark "Renoa" beside it in Instrument Sans 600 with -0.02em tracking. Keep clear space of half the mark on all sides, and never tint it with a pigment.

## Using it in Control Center
Control Center keeps its shadcn components (`src/components/ui`). The colour token names here match its semantic variables one to one: `background`, `foreground`, `card`, `primary`, `muted`, `accent`, `border`, `input`, `ring`, `sidebar-*`, `chart-*`, `destructive` and the `state-*` scale. Adopting the system means swapping values in `src/styles/design-system.css`, not rewriting pages. Pages add the Renoa pieces: part pigments, Tile, Rosette, PartChip, Note and the board. Plain HTML pages can use the `rn-` classes in `bundle.css`; the landing page (`design/landing.html`) carries its own copy of the styles it needs, so it stays one self-contained file.

The system sets how every control and surface looks. It does not yet define layout: the app shell (sidebar and page header), the structure of a settings page, or navigation between them.
