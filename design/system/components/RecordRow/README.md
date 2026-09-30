# RecordRow

Record rows are Renoa's list: ruled rows with a title, metadata and one state at the end. They replace card grids.

## You provide
A title that names the work in plain words, up to three metadata items (agent, part chip, time, an identifier in `code`), and at most one StateBadge.

## Rules
- Rows are separated by `border` hairlines, not boxed.
- A row that needs the owner gets `data-attention` (the `state-needs-you-wash`) and sorts first.
- Whole rows are links when they open detail. Hover washes with `accent`.
- Keep evidence (full IDs, timestamps, logs) in the detail view, not the row.
