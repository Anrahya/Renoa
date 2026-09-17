# UI sources

`src/components/ui/`, `src/hooks/use-mobile.ts`, and the Nova tokens in
`src/styles/design-system.css` were installed through shadcn CLI 4.21.0 from
its radix-nova registry on 2026-09-16. Upstream repository:
https://github.com/shadcn-ui/ui, observed source commit
`f5bb039c749afd2d111ad970dfaa9ab30bd90b3b`. License: MIT; full notice in
`public/licenses/shadcn-MIT.txt`. The registry is a published endpoint, not a Git-pinned
installation; the checked-in component files are Renoa's exact vendored source.

Local adaptations: split sidebar context/layout/menu to keep modules below
500 lines, removed its write-only preference cookie, changed SidebarInset to
a div to avoid nested main landmarks, replaced the Radix umbrella imports with
individual packages (the umbrella exposes an unrelated Select type error under
exactOptionalPropertyTypes), and scoped styling for incremental migration.

Layout reference: https://efferd.com/view/app-shell-5. Renoa's shell composes
upstream shadcn primitives around its own navigation and Host state; it does
not vendor the Efferd block's demo content or source. The MIT Efferd repository
https://github.com/shabanhr/efferd-ui at
`d0748f4fd12ba6553557f297d2ad4832a33d46cb` is the older collection and does not
contain app-shell-5.

Geist Variable 5.3.0 is bundled from `@fontsource-variable/geist` under OFL-1.1.
The font license ships at `public/licenses/geist-OFL.txt`.

On 2026-09-17, `native-select.tsx` was added through the same shadcn CLI and
radix-nova registry. Observed upstream commit:
`f5bb039c749afd2d111ad970dfaa9ab30bd90b3b`; MIT notice as above.

The Agents directory refresh uses the existing shadcn Item/Avatar/Tooltip
primitives. Efferd's https://efferd.com/view/dashboard-14 was inspected as a
reference for compact activity summaries; no source or assets from that block
were copied. Its billing charts and card grid are not part of this implementation.

The development-only agent-space map uses `@xyflow/react` 12.11.6 as a package
for pan, pinch, viewport fitting, and node rendering. MIT license, upstream
https://github.com/xyflow/xyflow at tag `@xyflow/react@12.11.6`, source commit
`0a1f9575b25679f2880175de8d3eae21aedde921`. Its notice ships at
`public/licenses/xyflow-MIT.txt`. The scene, contours, and nodes are Renoa code;
no example source was copied. The map is excluded from production builds.

TypeScript retains strict checking, including `exactOptionalPropertyTypes`,
for application source. `skipLibCheck` avoids errors inside xyflow's published
generic declarations under that flag; it does not suppress application errors.
