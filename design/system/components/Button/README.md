# Button

Buttons start an action. Primary is solid ink: Renoa has no hue accent, so ink is the colour of action.

## Variants
- **primary** (default): the one main action in a view. At most one per view.
- **secondary**: card fill with an `input` edge. Other actions beside a primary.
- **ghost**: text only. Cancel, dismiss, and actions inside dense rows.
- **destructive**: `state-failed` text and edge. Remove, delete, disconnect. Confirm in the page before it runs.

Sizes: `md` 40px (default), `sm` 32px for rows and toolbars.

## You provide
A label that says exactly what happens: "Swap model", "Approve Figma", "Remove plugin". Then report the result in the same words: "Model swapped".

## Rules
- In Control Center use the shadcn `Button` from `src/components/ui`; it reads the same tokens. `.rn-btn` is the reference rendering for plain HTML pages.
- Links in running text use `.rn-link`, not a ghost button.
- Focus: 2px `ring`, 2px offset.
