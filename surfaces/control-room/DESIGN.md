---
name: Renoa Web Surfaces
description: A dark charcoal and polished-gold system for one personal Host and its public introduction.
colors:
  charcoal: "#181714"
  ivory: "#eee8dc"
  muted-ivory: "#b4aa96"
  polished-gold: "#d3b66f"
  dark-rule: "#39352b"
  soft-charcoal: "#28251e"
  record-charcoal: "#22201b"
  footer-charcoal: "#211f1a"
  node-gold: "#8c774c"
  connection-gold: "#7b6946"
  idle-stone: "#a4977c"
  recorded-warning: "#efa18a"
  recorded-success: "#9bbc91"
  success-wash: "#253025"
  muted-wash: "#2b2820"
  banner-charcoal: "#24221c"
  error-wash: "#352520"
typography:
  display:
    fontFamily: "Manrope, sans-serif"
    fontSize: "clamp(58px, 5.65vw, 90px)"
    fontWeight: 500
    lineHeight: 1.08
    letterSpacing: "-0.04em"
  headline:
    fontFamily: "Manrope, sans-serif"
    fontSize: "clamp(34px, 3.5vw, 54px)"
    fontWeight: 500
    lineHeight: 1.18
    letterSpacing: "-0.04em"
  title:
    fontFamily: "Manrope, sans-serif"
    fontSize: "clamp(17px, 1.5vw, 22px)"
    fontWeight: 500
    lineHeight: 1.4
    letterSpacing: "-0.025em"
  body:
    fontFamily: "Manrope, sans-serif"
    fontSize: "16px"
    fontWeight: 400
    lineHeight: 1.6
  label:
    fontFamily: "Manrope, sans-serif"
    fontSize: "13px"
    fontWeight: 650
    lineHeight: 1.6
  host-page-title:
    fontFamily: "Manrope, sans-serif"
    fontSize: "2rem"
    fontWeight: 650
    lineHeight: 1.2
    letterSpacing: "-0.035em"
  host-section-title:
    fontFamily: "Manrope, sans-serif"
    fontSize: "1.125rem"
    fontWeight: 700
    lineHeight: 1.4
    letterSpacing: "-0.015em"
  host-body:
    fontFamily: "Manrope, sans-serif"
    fontSize: "0.9375rem"
    fontWeight: 400
    lineHeight: 1.6
  host-label:
    fontFamily: "Manrope, sans-serif"
    fontSize: "0.875rem"
    fontWeight: 600
    lineHeight: 1.6
  host-caption:
    fontFamily: "Manrope, sans-serif"
    fontSize: "0.8125rem"
    fontWeight: 400
    lineHeight: 1.65
rounded:
  status: "4px"
  tight: "5px"
  circle: "50%"
spacing:
  compact: "9px"
  control: "12px"
  host-control-gap: "16px"
  text-gap: "18px"
  mobile-gutter: "24px"
  host-mobile-gutter: "20px"
  host-row-gap: "24px"
  section-gap: "36px"
  host-content-gutter: "56px"
  page-gutter: "clamp(24px, 5.2vw, 88px)"
components:
  button-primary:
    backgroundColor: "{colors.polished-gold}"
    textColor: "{colors.charcoal}"
    typography: "{typography.label}"
    rounded: "{rounded.tight}"
    padding: "12px 18px"
  button-primary-hover:
    backgroundColor: "#e5cd92"
    textColor: "{colors.charcoal}"
  button-text:
    backgroundColor: "transparent"
    textColor: "{colors.ivory}"
    typography: "{typography.label}"
    padding: "0 0 9px"
  button-text-hover:
    textColor: "{colors.polished-gold}"
  button-icon:
    backgroundColor: "transparent"
    textColor: "{colors.muted-ivory}"
    rounded: "{rounded.circle}"
    size: "44px"
  host-button-primary:
    backgroundColor: "{colors.polished-gold}"
    textColor: "{colors.charcoal}"
    typography: "{typography.host-label}"
    rounded: "{rounded.tight}"
    padding: "12px 18px"
    height: "44px"
  host-button-primary-hover:
    backgroundColor: "#e5cd92"
    textColor: "{colors.charcoal}"
  host-button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.polished-gold}"
    typography: "{typography.host-label}"
    rounded: "{rounded.tight}"
    padding: "9px 16px"
    height: "44px"
  host-button-secondary-hover:
    backgroundColor: "{colors.soft-charcoal}"
    textColor: "{colors.polished-gold}"
  host-status:
    backgroundColor: "{colors.success-wash}"
    textColor: "{colors.recorded-success}"
    typography: "{typography.host-caption}"
    rounded: "{rounded.status}"
    padding: "3px 9px"
---

# Design System: Renoa Web Surfaces

## Overview

**Creative North Star: "The Golden Signal Field"**

Renoa's web system is one dark-only visual world. Warm charcoal holds every public and private surface; readable ivory carries the language; polished gold identifies the Host, selected concepts, links, focus, and action. Fine gold fibers and measured ownership lines keep the system open and connected without turning it into a dashboard of boxes.

The public homepage remains conceptual and contains no Host work data. The authenticated Host uses the same palette for recorded structure and controls: every persistent agent connects directly to the Host, then compact schedule and review branches reveal only admitted state. Generated identities add specific character without determining tools, ownership, or policy.

**Key Characteristics:**
- Dark charcoal is the only page ground; there is no light theme or theme toggle.
- Polished gold carries identity, interaction, selection, focus, and connection.
- Readable ivory and muted warm gray maintain hierarchy across both surfaces.
- One original Host emblem, two approved personal portraits, and twelve stable generic agent portraits replace placeholders.
- Conceptual homepage motion and observed Host updates share strict pause, visibility, and reduced-motion behavior.

## Colors

The palette is shared across the public homepage and authenticated Host. Near-black warm charcoal creates the field, ivory supports sustained reading, and polished gold supplies the single interactive and connective accent.

### Primary
- **Polished Gold:** Host identity, homepage emphasis, links, selected controls, focus outlines, filled actions, map sweeps, and active navigation.

### Neutral
- **Warm Charcoal:** The only page ground and the text color on filled gold actions.
- **Readable Ivory:** Primary headings, labels, records, and body text.
- **Muted Ivory:** Supporting copy, captions, quiet controls, and inactive navigation.
- **Dark Hairline:** Headers, sections, disclosures, and resting dividers.
- **Soft Charcoal:** Hover feedback, skip links, and quiet control surfaces.
- **Record Charcoal:** Expanded evidence bodies and text inputs.
- **Footer Charcoal:** The slightly lifted public footer field.
- **Node Gold:** Portrait borders, the connected-feed ring, and secondary-action strokes.
- **Connection Gold:** Direct Host ownership lines and roster branches.
- **Idle Stone:** The no-pending-work status dot.
- **Banner Charcoal:** Read-only and preview banners.

### Tertiary
- **Recorded Success:** Connected and enabled states use restrained sage on a deep green wash.
- **Recorded Warning:** Failed, interrupted, unavailable, and attention states use warm coral on a deep brown error wash.
- **Recorded Muted:** Paused and secondary state labels use a deeper neutral wash.

**The One Gold Rule.** Use Polished Gold as the single accent on both homepage and Host. Blue and light-surface tokens are superseded.

**The Recorded State Rule.** Sage and coral communicate states already present in Host records. They do not imply live telemetry.

## Typography

**Display Font:** Manrope (sans-serif fallback)

**Body Font:** Manrope (sans-serif fallback)

**Label/Mono Font:** Manrope for labels; `ui-monospace` for durable identifiers

Manrope is self-hosted as a Latin variable font across weights 400–800. Its open shapes support the homepage's oversized statements and the Host's compact operating density without changing voice. The font source and SIL Open Font License 1.1 are recorded in `public/assets/fonts/README.md` and `public/assets/fonts/OFL-Manrope.txt`. Font files use the Host's existing `/assets/` public route.

### Hierarchy
- **Display:** The public two-line first-viewport statement; medium weight, tight tracking, and balanced wrapping. On phones it changes to `clamp(52px, 10vw, 72px)`.
- **Headline:** The public closing idea statement, using the same medium weight and tight tracking at a smaller scale.
- **Title:** Public explanation headings use the fluid title role. Host page titles use a compact `2rem` role, reducing to `1.75rem` on phones.
- **Section title:** Host section headings are dense and firm; record titles sit one step below them.
- **Body:** Public page copy starts at `16px`; the Host uses `0.9375rem`. Descriptive text is constrained to readable line lengths between `65ch` and `75ch` where observed.
- **Label:** Public controls use the compact semibold label. Host navigation and actions use `0.875rem`; captions and record metadata use `0.8125rem`.

**The One Voice Rule.** Use Manrope for every web role; hierarchy comes from scale, weight, tracking, and space.

**The Identifier Rule.** Use the monospace fallback only for stored identifiers, digests, and revisions, never for headings or navigation.

## Layout

### Public Homepage

Content sits in a centered `1600px` maximum container with the fluid page gutter token. The first viewport uses an asymmetric two-column grid (`0.84fr 1.16fr`) and centers the statement beside the dominant `760 / 720` Canvas form. Explanation content uses two equal columns; the closing idea uses `1fr 3fr`, and its body divides into two columns.

At `1000px`, proportions tighten. At `720px`, the page becomes a single column, secondary navigation links hide, explanations and long-form copy stack, and circular marks shrink. At `1600px`, the hero receives a `730px` minimum height.

### Authenticated Host

The Host uses a centered `1120px` content column with `56px` side clearance, a `76px` ruled header, and a recurring `36px` section rhythm. Its System destination places a `112px` Host node beside a vertically flowing inventory of persistent agents inside an `880px` maximum stage. Measured curves connect the Host directly to each visible `64px` agent node; `created_by` remains provenance inside agent detail and never changes this layout. Compact schedule and unfinished-review branches indent beneath their target agent. The viewport grows to `min(72vh, 720px)` and scrolls without dropping agents. Search filters the complete inventory, while an explicit control reveals earlier identities.

The header exposes System, Agents, Work, and Library as separate hash destinations. System contains no work ledger, shared-resource summary, or per-agent resource counts. Agent routes retain their work, connections, automations, and review-policy controls.

At `900px`, the System stage narrows its Host column and connecting gap. At `640px`, the header wraps, content uses `20px` side gutters, the map viewport opens to full height, the Host becomes a compact horizontal introduction, and the agent inventory becomes an uncapped indented vertical map. Branch labels and timers wrap without hiding entries. Agent subnavigation becomes a two-column flow, Work selectors wrap with a `48px` minimum height, and other controls keep a `44px` minimum height.

**The Direct Host Ownership Rule.** Connect every persistent agent directly to the Host. Use creation history only as provenance in agent detail, never as operational grouping or hierarchy.

## Elevation & Depth

The homepage gets depth from overlapping transparent gold Canvas fibers and dark tonal layers. The Host stays flat for records and controls, then gives interactive agent portraits one focused lift: a `2px` rise with a compact deep shadow. Generated identity artwork carries its own polished highlights and modeled depth.

**The Flat Evidence Rule.** Records and branches expand within the document flow. Reserve lift for an interactive agent portrait under hover or keyboard focus.

## Shapes

The system is mostly borderless and rectangular. Tight `5px` corners belong to compact actions and inputs; `4px` corners belong to status labels. True circles identify the public system form, the `112px` Host ring around its `96px` emblem, `64px` agent portraits, and recorded attention dots. On phones the Host ring becomes `72px` with a `62px` emblem, and agent nodes become `48px`; agent-directory portraits use `44px`, while agent-detail portraits use `80px` and reduce to `64px` on phones. One-pixel measured curves express direct Host ownership; smaller elbow rules attach schedules to their target agent without creating card containers.

## Components

### Buttons
- **Filled primary:** Homepage entry and Host save actions use Polished Gold with Warm Charcoal text and a tight corner; hover brightens the gold.
- **Text action:** An open semibold ivory label sits on a one-pixel dark underline. Hover turns rule and text gold while its arrow travels `4px` over `240ms` with `cubic-bezier(0.16, 1, 0.3, 1)`.
- **Host secondary:** A transparent gold control with a Node Gold border is at least `44px` high; hover gains Soft Charcoal.
- **Focus:** Homepage actions use a `2px` Polished Gold outline with `7px` offset. Host controls and links use the same outline with `4px` offset.

### Navigation
- The wordmark remains heavy, tightly tracked ivory Manrope with its period in Polished Gold everywhere.
- Host header links label System, Agents, Work, and Library in muted semibold type, with a two-pixel gold bottom rule for the current route. Agent subnavigation repeats that active rule and exposes direct links to each responsibility.

### Host System Map
- An original polished-gold Renoa emblem with preserved transparency, a `6s` brightness breath, and a `24s` connected-feed ring anchors the map on charcoal. Every circular agent identity connects directly to the Host.
- Arcee and Soundwave retain their approved personal portraits. Every other agent receives one of twelve generated generic portraits through a stable hash of agent ID, so renaming or reordering never changes identity. The same portrait component appears in System, the agent directory, and agent detail.
- A small separate GitHub badge appears only when recorded repository policy assigns that agent to reviews. Portrait choice never determines role or runtime policy. The current Host contract exposes no Slack or Telegram binding telemetry, so the map shows neither badge.
- Generated WebPs retain prompt sidecars beside the shipped assets; `design/identities/provenance.json` records the main identity sources and derivatives.
- A single schedule appears as one direct link. Multiple schedules collapse into a compact group that reports total schedules, pending-run count or enabled state, and the next due timer; expansion reveals every ordered entry in a bounded scroll region.
- Unfinished review branches show repository, pull-request number, and the actual recorded stage: `Queued`, `Prepared`, `Publishing pending`, `Retry pending`, or `Attention`. The branch never converts stored review state into a worker heartbeat.
- Search filters all displayed persistent agents. The earlier-identities control expands the inventory explicitly. Creator breadcrumbs, creator branches, resource counts, the shared-library summary, and the work ledger do not belong on System.
- Curves are measured from rendered node bounds and redraw for resizing and scrolling. Offscreen agents remain in the inventory while their unseen connection paths are omitted. On phones, direct Host connections bend through the open vertical map.
- The ring turns once every `24s` and the emblem breathes once every `6s` only for a visible connected feed. Newly received session, review, or routine execution-record deltas trigger a `1.8s` sweep along the affected connection and around its agent; these sweeps never claim a worker heartbeat.
- One shared one-second clock updates live due times. Pause stops the ring, sweeps, and timers; hidden or offscreen views stop updates; reduced-motion preferences remove orbit, sweep, and clock-hand movement. Saved snapshots remain still. A development-only control labels synthetic activity as a motion demo and offers a return to the saved Host.

### Work View Selectors
- On the separate Work destination, four ruled selectors organize Attention, Unfinished, Schedules, and Reviews. Each uses a familiar icon, a factual count, and a two-pixel Polished Gold rule for the selected state.
- Only the selected record group is rendered in the Work flow. Empty copy names the absence directly; review history remains inside the Reviews selection.

### System Form
- The public native Canvas draws three 64-strand gold bundles from one center. Each strand is a subpixel warm-gold line with varying lightness and transparency; selection fades unselected bundles while the matching HTML control remains keyboard and touch operable.
- Motion breathes slowly and tilts subtly toward a non-touch pointer. Pause, page visibility, viewport visibility, and reduced-motion preferences stop animation while preserving the composition.

### Ruled Records
- Work, connection, plugin, and skill records are border-separated rows. Native disclosure controls reveal a tonal body with complete identifiers, timestamps, and evidence. State labels remain aligned right on wide screens and stack under titles on phones.
- Preview banners name the saved snapshot and date it. Write, save, and schedule-toggle actions stay disabled; review policy may open locally for inspection, while its Save remains disabled and the surface explains that changes require the live Host.

### Host Controls and States
- Schedule controls pause or resume a recorded routine and show pending or uncertain saves in place.
- Repository policy editing uses native checkboxes, explicit Save and Cancel actions, revision checks, and factual effects on future admissions. Status pills use green for enabled or scheduled and a neutral wash for paused.
- Model and tool recipe editing and cross-agent collaboration have no current control pattern; they remain future API work.

## Do's and Don'ts

### Do:
- **Do** keep the public homepage conceptual and free of Host work data while using the same dark charcoal, ivory, and gold visual system as the private Host.
- **Do** connect the Host directly to every persistent agent and attach schedules only to their recorded target agent.
- **Do** keep System structural; place work records and shared resources in their separate destinations.
- **Do** use stable record identities, timestamps, and explicit read-only language wherever a preview cannot act.
- **Do** preserve revision-safe schedule and repository policy controls on the live Host.
- **Do** preserve stable ID-based assignment across the twelve generic portraits and retain the named Arcee and Soundwave artwork; keep prompt sidecars and provenance with generated assets.
- **Do** preserve a still, complete homepage composition; keep saved Host snapshots still and suppress System motion when paused, hidden, offscreen, or reduced motion is preferred.

### Don't:
- **Don't** introduce card grids, decorative rooms, or a permanent inspector into the Host.
- **Don't** use portraits to infer tools, ownership, agent role, or policy; keep GitHub as a separate badge sourced from repository assignment.
- **Don't** add a light theme, blue accents, or a theme toggle.
- **Don't** invent Slack or Telegram binding badges when the Host snapshot does not expose that telemetry.
- **Don't** derive operational grouping from `created_by`, names, assignments, or creator history; creation is provenance only.
- **Don't** present record-delta sweeps, stored records, unfinished operations, schedule countdowns, or preview data as worker heartbeats or confirmed execution.
- **Don't** mix private Host data into the public homepage's conceptual Canvas.
- **Don't** imply model or tool recipe editing or cross-agent collaboration until those APIs and controls exist.
