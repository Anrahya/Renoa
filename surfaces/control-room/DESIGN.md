---
name: Renoa Web Surfaces
description: An open filament field with a precise operating interior for one personal Host.
colors:
  paper: "#f0f3fa"
  ink: "#182448"
  muted-ink: "#53617e"
  action-blue: "#294acc"
  rule: "#ccd5e7"
  hover-wash: "#e0e7fa"
  white: "#ffffff"
  host-rule: "#cbd5e7"
  host-soft: "#e4eafa"
  host-record: "#e8edf7"
  host-warning: "#9c3d26"
  host-success: "#286448"
  host-success-wash: "#e0eee6"
  host-muted-wash: "#e2e7f1"
  host-error-wash: "#f5e8e1"
  host-feed-line: "#9aadd5"
  host-ownership-line: "#a8b8da"
  host-agent-outline: "#91a5ce"
  host-quiet-dot: "#8392ad"
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
    backgroundColor: "{colors.ink}"
    textColor: "{colors.white}"
    typography: "{typography.label}"
    rounded: "{rounded.tight}"
    padding: "12px 18px"
  button-primary-hover:
    backgroundColor: "{colors.action-blue}"
    textColor: "{colors.white}"
  button-text:
    backgroundColor: "transparent"
    textColor: "{colors.ink}"
    typography: "{typography.label}"
    padding: "0 0 9px"
  button-text-hover:
    textColor: "{colors.action-blue}"
  button-icon:
    backgroundColor: "transparent"
    textColor: "{colors.muted-ink}"
    rounded: "{rounded.circle}"
    size: "44px"
  host-button-primary:
    backgroundColor: "{colors.action-blue}"
    textColor: "{colors.white}"
    typography: "{typography.host-label}"
    rounded: "{rounded.tight}"
    padding: "12px 18px"
    height: "44px"
  host-button-secondary:
    backgroundColor: "transparent"
    textColor: "{colors.action-blue}"
    typography: "{typography.host-label}"
    rounded: "{rounded.tight}"
    padding: "9px 16px"
    height: "44px"
  host-status:
    backgroundColor: "{colors.host-success-wash}"
    textColor: "{colors.host-success}"
    typography: "{typography.host-caption}"
    rounded: "{rounded.status}"
    padding: "3px 9px"
---

# Design System: Renoa Web Surfaces

## Overview

**Creative North Star: "The Open Filament Field"**

Renoa's web system presents a personal, modular AI system as one open field with a precise operating interior. The public homepage is spacious and conceptual: pale atmosphere, dark language, and a fine connected form whose visible parts meet at one center. The authenticated Host carries that same palette, typeface, rules, and exact interaction blue into a denser surface for operating real records.

The public and private surfaces stay distinct in purpose. The homepage explains without exposing data. The Host opens with a direct ownership map of the Host, every persistent agent, and each agent's target schedules. Work, Library, and detailed controls remain separate destinations that expand evidence only when the owner asks for it.

**Key Characteristics:**
- Pale blue continuous ground with deep indigo type.
- One clear blue for emphasis, selection, focus, and action.
- Self-hosted Manrope from large public statements through compact Host labels.
- Fine rules, circular agent nodes, and open flow instead of decorative chrome.
- Conceptual motion on the homepage; restrained state motion and reduced-motion support in the Host.

## Colors

The shared palette is a cool, low-contrast field anchored by dark indigo ink and one clear action blue. The Host adds only the status and tonal surfaces required to operate real records.

### Primary
- **Action Blue:** Emphasizes selected words, active navigation, focus outlines, links, primary Host actions, and interaction states.

### Neutral
- **Open Paper:** The uninterrupted ground for both public and authenticated surfaces.
- **Deep Ink:** Headlines, wordmarks, and primary text.
- **Muted Ink:** Supporting copy, labels, captions, quiet controls, and inactive navigation.
- **Homepage Hairline:** Public-page dividers and resting control borders.
- **Host Hairline:** Host headers, sections, records, and history dividers.
- **Hover Wash:** The public circular-control hover fill.
- **Host Soft Wash:** The Host icon and secondary-action hover fill.
- **Record Wash:** The expanded Host record body.
- **Connected Feed Line:** The fine ring around a connected live Host.
- **Host Ownership Line:** Direct measured curves from the Host to visible persistent agents.
- **Agent Outline:** Resting borders around persistent agent identities.
- **Quiet Dot:** The compact no-pending-work indicator.
- **White:** Text on filled actions and selected marks.

### Tertiary
- **Recorded Success:** Connected, enabled, and scheduled states; its pale wash backs compact status labels.
- **Recorded Warning:** Failed, interrupted, unavailable, and attention states; its pale wash is reserved for error banners.
- **Recorded Muted:** Paused and secondary status labels use a cool neutral wash.

**The One Blue Rule.** Use Action Blue for interaction and selection across both surfaces; leave the ground and most content neutral.

**The Recorded State Rule.** Green and rust communicate states already present in Host records. They do not imply live telemetry.

## Typography

**Display Font:** Manrope (sans-serif fallback)

**Body Font:** Manrope (sans-serif fallback)

**Label/Mono Font:** Manrope for labels; `ui-monospace` for durable identifiers

Manrope is self-hosted as a Latin variable font across weights 400–800. Its open shapes support the homepage's oversized statements and the Host's compact operating density without changing voice. The font source and SIL Open Font License 1.1 are recorded in `public/fonts/README.md` and `public/fonts/OFL-Manrope.txt`.

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

The Host uses a centered `1120px` content column with `56px` side clearance, a `76px` ruled header, and a recurring `36px` section rhythm. Its System destination places a `112px` Host node beside a vertically flowing inventory of persistent agents inside an `880px` maximum stage. Measured curves connect the Host directly to each visible `64px` agent node; `created_by` remains provenance inside agent detail and never changes this layout. Target schedules indent beneath their agent with a compact name, state icon, and tabular due time. The viewport grows to `min(72vh, 720px)` and scrolls without dropping agents. Search filters the complete inventory, while an explicit control reveals earlier identities.

The header exposes System, Agents, Work, and Library as separate hash destinations. System contains no work ledger, shared-resource summary, or per-agent resource counts. Agent routes retain their work, connections, automations, and review-policy controls.

At `900px`, the System stage narrows its Host column and connecting gap. At `640px`, the header wraps, content uses `20px` side gutters, the map viewport opens to full height, the Host becomes a compact horizontal introduction, and the agent inventory becomes an uncapped indented vertical map. Schedule timers wrap beneath long names. Agent subnavigation becomes a two-column flow, Work selectors wrap with a `48px` minimum height, and other controls keep a `44px` minimum height.

**The Direct Host Ownership Rule.** Connect every persistent agent directly to the Host. Use creation history only as provenance in agent detail, never as operational grouping or hierarchy.

## Elevation & Depth

Both surfaces use no shadows. The homepage gets depth from translucent Canvas fibers. The Host uses one-pixel rules and tonal fills: expanded records, status labels, preview banners, and error banners sit on quiet washes without appearing elevated.

**The Flat Evidence Rule.** Records expand within the document flow. Use tonal change and rules to reveal evidence, never floating panels or permanent inspectors.

## Shapes

The system is mostly borderless and rectangular. Tight `5px` corners belong to compact actions and inputs; `4px` corners belong to status labels. True circles identify the public system form, the `112px` Host ring with its `84px` core, `64px` agent nodes, and recorded attention dots. On phones the Host ring becomes `72px` with a `56px` core, and agent nodes become `48px`. One-pixel measured curves express direct Host ownership; smaller elbow rules attach schedules to their target agent without creating card containers.

## Components

### Buttons
- **Public primary:** The compact Host entry link uses Deep Ink, white text, and a tight corner; hover changes the fill to Action Blue. Its label is `Preview Host` in development and `Open Host` in production.
- **Public text action:** An open semibold label sits on a one-pixel underline. Hover turns rule and text blue while its arrow travels `4px` over `240ms` with `cubic-bezier(0.16, 1, 0.3, 1)`.
- **Host primary:** A filled Action Blue control is at least `44px` high; hover deepens the blue.
- **Host secondary:** A transparent, blue, one-pixel control is at least `44px` high; hover gains Host Soft Wash.
- **Focus:** Public actions use a `2px` Action Blue outline with `7px` offset. Host controls and links use the same outline with `4px` offset.

### Navigation
- The wordmark remains heavy, tightly tracked Manrope with its period in Action Blue.
- Host header links label the four destinations System, Agents, Work, and Library in muted semibold type, with a two-pixel bottom rule for the current hash route. Agent subnavigation repeats that active rule and exposes direct links to each responsibility.

### Host System Map
- A filled Host core and orbiting connected-feed ring anchor the map. Every outlined agent node connects directly to the Host; initials identify ordinary agents, while the GitHub mark appears only when repository policy assigns that agent to reviews.
- Compact indicators distinguish quiet, unfinished, and attention states without counts. Each node links to agent detail and may carry one or more indented schedules with a ticking due time or the factual state `Pending`, `Paused`, `Scheduled`, or `Due`.
- Search filters all displayed persistent agents. The earlier-identities control expands the inventory explicitly. Creator breadcrumbs, creator branches, resource counts, the shared-library summary, and the work ledger do not belong on System.
- Curves are measured from rendered node bounds and redraw for resizing and scrolling. Offscreen agents remain in the inventory while their unseen connection paths are omitted. On phones, direct Host connections bend through the open vertical map.
- The ring turns once every `24s` only for a visible connected feed. Newly received session, review, or routine execution-record deltas trigger a `1.8s` sweep along the affected connection and around its agent; these sweeps never claim a worker heartbeat.
- One shared one-second clock updates live due times. Pause stops the ring, sweeps, and timers; hidden or offscreen views stop updates; reduced-motion preferences remove orbit, sweep, and clock-hand movement. Saved snapshots remain still. A development-only control labels synthetic activity as a motion demo and offers a return to the saved Host.

### Work View Selectors
- On the separate Work destination, four ruled selectors organize Attention, Unfinished, Schedules, and Reviews. Each uses a familiar icon, a factual count, and a two-pixel Action Blue rule for the selected state.
- Only the selected record group is rendered in the Work flow. Empty copy names the absence directly; review history remains inside the Reviews selection.

### System Form
- The public native Canvas draws three 64-strand bundles from one center. Each strand is a subpixel indigo-blue line with varying lightness and transparency; selection fades unselected bundles while the matching HTML control remains keyboard and touch operable.
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
- **Do** preserve the public homepage as a no-data product introduction and keep the private Host behind authentication.
- **Do** connect the Host directly to every persistent agent and attach schedules only to their recorded target agent.
- **Do** keep System structural; place work records and shared resources in their separate destinations.
- **Do** use stable record identities, timestamps, and explicit read-only language wherever a preview cannot act.
- **Do** preserve revision-safe schedule and repository policy controls on the live Host.
- **Do** preserve a still, complete homepage composition; keep saved Host snapshots still and suppress System motion when paused, hidden, offscreen, or reduced motion is preferred.

### Don't:
- **Don't** introduce card grids, portraits, decorative rooms, or a permanent inspector into the Host.
- **Don't** derive operational grouping from `created_by`, names, assignments, or creator history; creation is provenance only.
- **Don't** present record-delta sweeps, stored records, unfinished operations, schedule countdowns, or preview data as worker heartbeats or confirmed execution.
- **Don't** mix private Host data into the public homepage's conceptual Canvas.
- **Don't** imply model or tool recipe editing or cross-agent collaboration until those APIs and controls exist.
