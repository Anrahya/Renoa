# Field

A field is a label, a control and optional help: input, select or textarea.

## You provide
A label in sentence case, a stable `id`, an optional hint, and an error that says what went wrong and how to fix it.

## Rules
- Labels sit above the control, never inside it. Placeholders are examples, not labels.
- Identifiers (presets, IDs, repository names) use `.rn-mono`.
- Errors replace the hint, set `data-invalid` on the field and `aria-invalid` on the control, and never apologise: "Agent A can't see this channel. Invite it to the server, then try again."
- Control edges use `input` (3:1); hover and focus go to `foreground`.
