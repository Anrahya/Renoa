# PartChip

A part chip names a part inline, with a small hex in the part's pigment so the name and the tile read as the same thing.

## Use it for
Section headings on the Configure page, breadcrumbs, table cells and change notes ("Swapped Model to v4"). Anywhere text refers to a part.

## You provide
`data-part` (one of model, loop, context, tools, skills, profile) or `data-kind="plugin"` / `"core"`, the part name as text, and an optional `.rn-ver` with a version or identifier in mono.

## Rules
- Use the part's own name: Model, Workflow, Context, Tools, Skills, Profile. Do not invent synonyms like Brain or Memory.
- The hex is decoration for sighted readers; the word carries the meaning. Never show the hex alone.
