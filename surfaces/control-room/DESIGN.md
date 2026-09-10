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

The public and private surfaces stay distinct in purpose. The homepage explains without exposing data. The Host is an agent-focused flow of recorded creation relationships, shared resources, selectable work summaries, and ruled records; it avoids a card grid and expands details only when the owner asks for evidence or controls.

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

The Host uses a centered `1120px` content column with `56px` side clearance, a `76px` ruled header, and a recurring `36px` section rhythm. The first view flows from page heading to a searchable creation map, the shared library, four recorded-work selectors, and the selected records. A focused creator and its direct descendants occupy a two-column branch inside an `840px` maximum map; groups larger than the `520px` viewport scroll without dropping agents. Breadcrumbs move among recorded creator groups, and search reaches agents inside unopened groups. Hash navigation links directly to overview, agents, shared library, an individual agent, and that agent's work, connections, automations, or review policy.

At `900px`, gutters and branch spacing tighten. At `640px`, the header wraps, content uses `20px` side gutters, the map toolbar stacks, the branch becomes an uncapped indented tree, records stack their state under the title, and agent subnavigation becomes a two-column flow. Work selectors wrap and keep a `48px` minimum height; other controls keep a `44px` minimum height.

**The Agent-First Flow Rule.** Lead with named agents, recorded creator relationships, and real resource counts, then unfold selected work and evidence in reading order; do not convert the Host into a boxed metric grid.

## Elevation & Depth

Both surfaces use no shadows. The homepage gets depth from translucent Canvas fibers. The Host uses one-pixel rules and tonal fills: expanded records, status labels, preview banners, and error banners sit on quiet washes without appearing elevated.

**The Flat Evidence Rule.** Records expand within the document flow. Use tonal change and rules to reveal evidence, never floating panels or permanent inspectors.

## Shapes

The system is mostly borderless and rectangular. Tight `5px` corners belong to compact actions and inputs; `4px` corners belong to status labels. True circles identify the public system form, Host and agent nodes, and recorded attention dots. Focused map nodes grow from `56px` to `84px`; mobile nodes reduce to `44px`, with a `64px` focused node. One-pixel rules describe hierarchy and recorded creation without creating card containers.

## Components

### Buttons
- **Public primary:** The compact Host entry link uses Deep Ink, white text, and a tight corner; hover changes the fill to Action Blue. Its label is `Preview Host` in development and `Open Host` in production.
- **Public text action:** An open semibold label sits on a one-pixel underline. Hover turns rule and text blue while its arrow travels `4px` over `240ms` with `cubic-bezier(0.16, 1, 0.3, 1)`.
- **Host primary:** A filled Action Blue control is at least `44px` high; hover deepens the blue.
- **Host secondary:** A transparent, blue, one-pixel control is at least `44px` high; hover gains Host Soft Wash.
- **Focus:** Public actions use a `2px` Action Blue outline with `7px` offset. Host controls and links use the same outline with `4px` offset.

### Navigation
- The wordmark remains heavy, tightly tracked Manrope with its period in Action Blue.
- Host header links use muted semibold labels and a two-pixel bottom rule for the current hash route. Agent subnavigation repeats that active rule and exposes direct links to each responsibility.

### Agent Creation Map
- A focused creator uses a filled `84px` circular node and branches to outlined `56px` child nodes. Initials identify ordinary agents; the GitHub mark appears only when repository policy assigns that agent to reviews. Rust attention marks and factual state text surface recorded problems.
- Each node links to the agent and exposes three labeled icon counts: selected MCP connections, recorded sessions, and enabled schedules. These counts describe stored resources and records; they do not imply chat bindings, live presence, or agent-to-agent communication.
- Breadcrumbs focus one creator group at a time. Search spans unopened groups, the earlier-identity control expands the inventory explicitly, and child-group links preserve branch context.
- Desktop groups scroll inside a `520px` maximum map without record caps. On phones, the same creator links become an uncapped indented tree.

### Recorded Work Selectors
- Four ruled selectors replace the stacked overview ledger: Attention, Unfinished, Schedules, and Reviews. Each uses a familiar icon, a factual count, and a two-pixel Action Blue rule for the selected state.
- Only the selected record group is rendered in the overview flow. Empty copy names the absence directly; review history remains inside the Reviews selection.

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
- **Do** keep the creation map, shared resources, selected work records, and controls in one flowing reading order.
- **Do** derive GitHub review roles, resource counts, and creator branches from recorded Host data.
- **Do** use stable record identities, timestamps, and explicit read-only language wherever a preview cannot act.
- **Do** preserve revision-safe schedule and repository policy controls on the live Host.
- **Do** preserve a still, complete homepage composition and remove Host transitions for reduced-motion preferences.

### Don't:
- **Don't** introduce card grids, portraits, decorative rooms, or a permanent inspector into the Host.
- **Don't** use creation connectors or resource icons to imply agent authority, live communication, shared thoughts, or a surface binding that the Host contract does not record.
- **Don't** present stored records, unfinished operations, or preview data as live agent telemetry.
- **Don't** mix private Host data into the public homepage's conceptual Canvas.
- **Don't** imply model or tool recipe editing or cross-agent collaboration until those APIs and controls exist.
