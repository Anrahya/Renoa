# Control Room design QA

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
