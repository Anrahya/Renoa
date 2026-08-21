# acp-components provenance

- Source: <https://github.com/zvzuola/acp-components>
- Revision: `1708c20274c9f15ee3a072009e5ca9fd3b71a9de`
- Revision date: 2026-08-05
- Audited: 2026-08-21
- License: MIT, reproduced in `LICENSE`

Renoa vendors the upstream `packages/core` package and its inherited root
TypeScript configuration without source changes. Generated output and
`node_modules` are omitted. The React package is not vendored: Renoa supplies
its own presentation while retaining upstream's ACP client, provider, actions,
and Zustand stores.

The upstream Tauri example informed these adapted Renoa-owned files:

- `ui/desktop/src/acp/tauriTransport.ts`, adapted from
  `examples/tauri/src/tauriIpcTransport.ts`.
- `ui/desktop/src-tauri/src/bridge.rs`, adapted from
  `examples/tauri/src-tauri/src/main.rs`.

Both adaptations remain covered by the upstream MIT license. Renoa narrows the
native bridge to the fixed `renoa-agent acp` process, registers listeners before
launch to avoid losing early protocol messages, removes unrelated workspace and
file APIs, and splits native responsibilities into reviewable modules.

## Audit result

At the recorded revision, the two upstream packages contain 20,020 TypeScript
and TSX lines. All 285 upstream tests pass (186 core and 99 React), and both
packages build and lint. The known package export-condition warnings and three
i18next test diagnostics remain upstream. The npm packages are still version
`0.1.0`, published 2026-05-28, and predate this revision.

## Synchronizing

Clone or fetch the source revision outside this repository, then compare and
replace only the imported paths:

```sh
git clone https://github.com/zvzuola/acp-components.git /tmp/acp-components
git -C /tmp/acp-components checkout 1708c20274c9f15ee3a072009e5ca9fd3b71a9de
diff -ru --exclude=dist --exclude=node_modules \
  /tmp/acp-components/packages/core packages/core
```

Before updating the recorded revision, run the upstream tests, review protocol
and store changes, and update this provenance record. Renoa-owned presentation
changes stay outside the vendor directory.
