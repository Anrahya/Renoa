# StateBadge

A state badge reports what an agent, task or routine is doing, in one of five words, with a dot.

## States
Ordered by what the owner can do about them:

| state | token | when |
| --- | --- | --- |
| Needs you | `state-needs-you` (filled) | Waiting on the owner: an approval, an answer, a login. The only state you can act on, so it wears the action colour. |
| Failed | `state-failed` on `state-failed-wash` | Work that actually broke. |
| Running | `state-running` (dot pulses) | In progress, assigned, or triggered. |
| Idle | `state-idle` (hollow dot) | Nothing pending. |
| Done | `muted-foreground` with a check | A recorded outcome. |

## You provide
`data-state` and the word. Keep the word even when space is tight; colour alone never carries a state.

## Rules
- A circle always means live state. Do not use dots for categories or parts.
- Show at most one badge per row, at the row end.
- Only report states the Host has recorded. A schedule countdown or a stored record is not Running.
