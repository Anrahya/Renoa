# Note

A note says what one tile is doing, right beside it. It is a small card with a mono eyebrow and one sentence, joined to its tile by a thin leader line. Tiles show structure; notes show what is happening to it.

## Use it for
- Narrating a diagram as it moves: "Step 3 · Needs you / Approve the Figma connection?"
- Showing a record next to the thing that keeps it: a task's saved steps beside the core, as `rn-note-rows`.
- Marking a path that is not shipped yet: add `<span class="rn-tag">Being built</span>` to the head.

## You provide
An eyebrow (where or what: "Step 2 · Provider docs", "RCP · Task", "Discord · You"), one short sentence, and optional rows of `[label, value]`.

## Rules
- One note at a time. When the action moves to another tile, the note moves with it; it does not multiply.
- Put it in the emptiest spot near its tile. The leader starts on the tile with a dot and ends at the nearest edge of the note. It may cross tiles, never text.
- Keep the sentence under about 12 words, in the same voice as the page: plain, active, no exclamation marks.
- Quotes are for what someone said in an app ("“Is PR #42 ready to merge?”"). Everything else is narration.
- Anything not built yet is drawn dashed or hollow and carries the Being built tag. Remove the tag the day it ships.

## Pointing from text
In running text, `rn-cue` marks words that point at the board with a dotted underline. Hovering or focusing them draws a tether to their tile. The words being shown right now get `aria-current` and a highlight. Selecting them plays that moment again. One cue is current at a time.
