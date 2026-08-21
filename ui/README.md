# Renoa desktop surface

This workspace contains Renoa's first desktop surface. The React application
talks only to the `renoa-agent acp` process. The Tauri shell owns process
launch and newline-delimited JSON-RPC transport; it does not call the Renoa
kernel, harness, model, or tools directly.

## Layout

- `desktop/`: Renoa-owned React presentation and the thin Tauri shell.
- `vendor/acp-components/`: pinned upstream ACP client and Zustand state core.

The desktop app imports the vendored core source through a Vite and TypeScript
alias. This keeps the upstream snapshot unchanged and makes upstream diffs
mechanical. See `vendor/acp-components/UPSTREAM.md` for provenance and sync
instructions.

The webview stores only the last session UUID and workspace needed for the
Resume button. Transcript history is replayed by Renoa over ACP from gapless
kernel events; browser local storage is never a competing conversation store.
The Tauri shell places the ACP child and everything it launches in one process
group, then terminates and reaps that group on disconnect or application exit.

## Development

From this directory:

```sh
pnpm install
pnpm check
pnpm desktop:dev:local
```

On Fedora, install Tauri's native build inputs once:

```sh
sudo dnf install gtk3-devel webkit2gtk4.1-devel \
  libappindicator-gtk3-devel librsvg2-devel
```

These are the Fedora packages from the
[Tauri prerequisites](https://v2.tauri.app/start/prerequisites/). They are not
installed by `pnpm install`.

Authenticate SuperGrok once (and repeat this if xAI revokes the refresh token):

```sh
pnpm --dir ../nodes/pi install --frozen-lockfile --ignore-scripts
RENOA_PI_AUTH_STORE="$HOME/.config/renoa/pi-auth.sqlite" \
  pnpm --dir ../nodes/pi auth:xai
```

`desktop:dev:local` installs the locked JavaScript dependencies, builds the Pi
bridge and `renoa-agent`, then starts Tauri. It defaults to the local SuperGrok
credential store at `~/.config/renoa/pi-auth.sqlite` with `xai/grok-4.6`.
Override any `RENOA_PI_*` variable before launch to use another configured
provider or model. `RENOA_AGENT_BIN` can override the agent executable.

`desktop:dev` is the lower-level command for a preconfigured environment.

For browser-only presentation work, run `pnpm --filter @renoa/desktop dev`
and open `http://localhost:5173/?fixture`. This development-only route uses the
same deterministic ACP fixture as the integration tests; production builds
always use the Tauri transport.

The ACP process still requires the provider environment documented in
`../docs/acp-v1.md`.
