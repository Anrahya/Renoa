# Tabs

Ruled tabs switch between views of one thing, like an agent's Overview, Configure, Automations and Activity.

## You provide
Tab labels (one or two words), the selected tab, and an optional `.rn-count` for a factual count.

## Rules
- The current tab gets a 2px `primary` rule on the hairline; inactive tabs are `muted-foreground`.
- Tabs never wrap: on phones the row scrolls sideways inside itself.
- Use `aria-current="page"` instead of `aria-selected` when each tab is its own URL (hash routes).
