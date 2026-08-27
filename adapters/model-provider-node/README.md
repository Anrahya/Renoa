# @renoa/model-provider

Renoa-owned TypeScript process that maps the provider-neutral model contract to
xAI and OpenCode Go. Architecture: [`docs/model-provider-v0.md`](../../docs/model-provider-v0.md).
Adapted protocol sources: [`UPSTREAM.md`](UPSTREAM.md).

## Authenticate

Existing credentials stay in `$HOME/.config/renoa/pi-auth.sqlite`.

```sh
mkdir -p ~/.config/renoa
chmod 700 ~/.config/renoa
export RENOA_MODEL_AUTH_STORE="$HOME/.config/renoa/pi-auth.sqlite"
pnpm install --frozen-lockfile --ignore-scripts
pnpm auth:xai            # SuperGrok device-code OAuth
# printf '%s' "$OPENCODE_API_KEY" | pnpm auth:opencode-go
```

Auth CLIs print actionable errors and never print secrets.

## Build and test

```sh
pnpm install --frozen-lockfile --ignore-scripts
pnpm build
pnpm test
```

Tests use loopback fake HTTP servers. They do not need internet or live keys.

## Configure Alpha / ACP

Point Renoa at the compiled entrypoint:

```sh
export RENOA_MODEL_BRIDGE="/absolute/path/to/adapters/model-provider-node/dist/src/main.js"
export RENOA_MODEL_AUTH_STORE="$HOME/.config/renoa/pi-auth.sqlite"
export RENOA_MODEL_PROVIDERS=xai,opencode-go
export RENOA_MODEL_PROVIDER=opencode-go
export RENOA_MODEL=deepseek-v4-pro
```

ACP shows the combined catalog and stores the chosen provider and model with
each session. `RENOA_MODEL_PROVIDER` and `RENOA_MODEL` only choose the default
for a new session. Omit `RENOA_MODEL_PROVIDERS` to enable only that default
provider. OpenCode Go availability refreshes at catalog discovery; Renoa keeps
a validated last-known-good cache and the bundled catalog as offline fallbacks.

The process reads `RENOA_MODEL_ACTION` (`catalog` | `describe` | `stream`) from
the Rust host. Do not set it by hand for normal Alpha use.
