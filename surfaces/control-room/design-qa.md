## Cloud Host profile integration — 2026-09-16

The production agent route now uses the open profile composition with the existing Host observation contract. Connections join by exact profile; schedules, reviews, sessions and policies join by exact agent ID. No model, instruction, preset, tool-selection or sharing settings are invented. Default agent links open the overview; existing work/connections/automations/policy links open their detail drawer.

Browser checks used the labelled saved VPS snapshot at `/?preview`, including Soundwave's review policy. Outside click and Escape dismiss the native modal and restore opener focus. Drawer opening updates the section URL; browser Back closes it, reloading a section reopens it, and Escape restores the overview URL. An edited policy checkbox survived dismissal/reopening; Cancel discarded the preview draft. Compact 390px layout and full-width drawer had no horizontal overflow. Live server writes were not performed during this visual check; existing mutation tests cover request identity, revision conflicts and uncertain receipts. The production build uses the existing authenticated Host feed and real routine/review controls. Deployed on 2026-09-16 as frontend release `4a960c6-panel-78e73c0ccd3f` to `https://renoa.live`. Public and loopback shell/JS/CSS hashes match the built files, anonymous Host access remains 401, and all six service PIDs stayed unchanged. The live Host UUID and inventory counts matched before/after. The browser rendered the expected pairing screen; an authenticated browser mutation was not exercised. Exactly one previous-release backup remains, and staging uploads were removed.

Validation: TypeScript, 59 frontend tests, production build, four Sites tests, Rust formatting, workspace Clippy with warnings denied, and all workspace tests passed. The new projection test proves agent isolation, paused-schedule exclusion, admission-order review selection and unchanged source records. The saved design preview remains separate and development-only. A later workspace rerun hit an intermittent Slack restart-test lease contention (`uncertain_middle_chunk_blocks_the_remaining_suffix_across_restart`); its isolated rerun and subsequent full workspace rerun passed. No Slack code was changed.

# Control Room design QA

## Agent profile — interaction revision, 16 September 2026

The owner's latest feedback supersedes the initial raster where interactions differ. Keep the open composition and dark palette; simplify editing and remove non-actionable branches.

- Final evidence: `design/agent-profile-desktop.png`, `design/agent-profile-drawer.png`, and `design/agent-profile-mobile.png`.
- Historical visual comparison: `design/agent-profile-comparison.webp`, reference left and revised overview right, each normalized to 800 px high with aspect ratio preserved.
- Desktop was checked at 1309 × 931 CSS pixels and mobile at 390 × 844. The browser's temporary viewport override was reset afterward.
- Runnable surface: development-only `/?agent-design`; example data and edits stay in the tab. Production excludes the prototype.

**Changes established by interaction checks**

- Customize opens Basics with name, role, purpose, and an overview of the other building blocks. Model, Instructions, and Capabilities are sections of the same editor. Direct page shortcuts select the corresponding section.
- Outside click, Close, and Escape dismiss the panel. An edited name remained unsaved on the overview and was restored by Resume unsaved changes. Save commits the draft; Discard removes it. No extra confirmation blocks dismissal.
- Tools, skills, and MCP connections are stable checkboxes inside expandable groups. Unchecking Google Drive and checking GitHub preserved both library rows. Toggling Read files off and on returned to No changes. Search, empty results, and Clear search worked.
- A Coding preset replaced only draft purpose and capabilities, with its effect described before application. A later 4,096-token output limit and the preset saved together and appeared on the overview. Zero-token input disabled Save and presented a useful error; Discard restored the original limit.
- Saving an enabled daily schedule left a separate agent draft unsaved. Deterministic tests prove that older drafts cannot overwrite unrelated saved configuration or schedules.
- Generic Context copy was removed. Runtime became a secondary identity detail, removing an unnecessary connector and avoiding collisions with longer identities. Future peer access and context-sharing policy remain unimplemented.

**Visual and accessibility checks**

- Desktop and mobile retain clear grouping, readable labels, native form controls, visible focus, scrollable panel content, and reachable footer actions. No horizontal overflow was measured on the page or dialog at 390 px.
- The manual design detector reported advisory typography/palette differences. New editor metadata was moved to 13 px and the hover border uses the existing gold token. Existing map scale choices remain intentional.
- No browser warnings or errors appeared. No actionable P0/P1/P2 findings remain in the inspected preview flows.

**Scope and limits**

Live Host configuration, actual model-limit validation, account authorization, presets, and cross-agent sharing require backend support. These controls are an explicitly labeled interactive design. Agent drafts and schedule drafts are separate to prevent unintended combined saves. Refreshing the page restores the original examples.

final result: passed

## Previous task-console verification

- Source visual truth: `design/tasks-reference.png`
- Rendered implementation: `design/implementation-desktop.png`
- Full comparison: `design/comparison-desktop.webp`
- Focused task-rail comparison: `design/comparison-task-rail.webp`
- Browser state: development-only design preview with seven task summaries and durable event records shaped exactly like the current RCP contract
- CSS viewport: 1280 × 720 at device pixel ratio 1
- Source pixels: 1487 × 1058
- Implementation pixels: 1265 × 779 full-page capture
- Normalization: both full views were scaled to 876 px high with aspect ratio preserved before side-by-side comparison. The focused rails were cropped from their native captures and scaled to a common 360 px width.

**Findings**

- No actionable P0, P1, or P2 differences remain.
- [P3] The implementation uses the platform sans-serif stack while the mock's exact typeface is unknown. Weight, scale, wrapping, and hierarchy are visually equivalent at the inspected sizes.
- [P3] The mock has bespoke 3D art for every task pod. The implementation uses one generated isometric console and compact Phosphor target icons so seven tasks remain readable. This is intentional: current RCP task summaries expose only `taskId` and `target`, so per-agent art would invent identity the Host did not send.

**Required fidelity surfaces**

- Fonts and typography: neutral sans-serif hierarchy, compact metadata weights, truncation, and line lengths match the source direction. No clipping appeared at desktop or 390 px width.
- Spacing and layout rhythm: the task rail, open console, metadata strip, and actions preserve the source composition. The rail is deliberately denser to support six or seven tasks without turning the page into a wall of cards.
- Colors and visual tokens: white and cool-gray surfaces, restrained blue focus, and green/red semantic states consistently follow the source palette with accessible text contrast.
- Image quality and asset fidelity: the generated console asset is sharp at its rendered crop and matches the white isometric hardware direction. Visible interface icons come from Phosphor; no handcrafted SVG, emoji, placeholder art, or CSS illustration replaces source imagery.
- Copy and content: task targets, state, timestamps, errors, counts, and history come from real RCP fields. Office, Library, and Settings are disabled because the current Host does not expose those directories yet.

**Comparison history**

- First pass: no P0/P1/P2 findings. One P3 issue was found: repeating the same code icon weakened at-a-glance scanning.
- Fix: workspace, Telegram, and service targets now use distinct source-derived Phosphor icons.
- Post-fix evidence: `design/implementation-desktop.png`, `design/comparison-desktop.webp`, and `design/comparison-task-rail.webp`. A fresh browser tab reported no warnings or errors.

**Interactions checked**

- Selected a different task and confirmed its projected state changed.
- Opened and closed authoritative task history.
- Opened the continuation composer, submitted text, and observed the durable event count update.
- Inspected the 390 × 844 responsive layout and restored the desktop viewport.
- Verified the production build and Sites packaging.

**Residual test gap**

- A live biometric ceremony was not run because QA had no one-use passkey bootstrap. The rendered registration and unlock forms were inspected; identity endpoint shapes and ticket handling are covered by the repository's deterministic RCP tests.

**Implementation Checklist**

- [x] Preserve the selected white isometric visual direction.
- [x] Keep seven tasks glanceable and expandable.
- [x] Use real Host/RCP data only in production mode.
- [x] Persist projection before cursor advancement and command before transmission.
- [x] Keep passkey tickets in memory only.
- [x] Verify desktop, narrow layout, history, task selection, and continuation.

final result: passed
