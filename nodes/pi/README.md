# Renoa Pi node

This is the first non-Rust harness attached to RCP. It runs Pi's agent loop,
stores admitted commands, conversation context, and execution events in local
SQLite, and reconnects to the Renoa coordinator without stopping active work.
It uses Pi's core SDK packages directly; Pi's CLI and TUI are not dependencies.

Pi's prompt, target, model, and tools are local node configuration. RCP sends
only a task identity and command. A workspace in `read` mode exposes Pi's
packaged read tool. `read_write` exposes its read, write, and edit tools. With
no workspace configuration, Pi receives no tools. Bash, network tools, and
interactive approvals are not implemented.

## Authenticate with SuperGrok

Renoa uses Pi's xAI device-code OAuth directly; Pi's TUI is not involved. Keep
the credential database outside the repository and protect its parent directory:

```sh
mkdir -p ~/.config/renoa
chmod 700 ~/.config/renoa
export RENOA_PI_AUTH_STORE="$HOME/.config/renoa/pi-auth.sqlite"
pnpm install --frozen-lockfile --ignore-scripts
pnpm auth:xai
```

Open the printed xAI URL and enter its short code. Renoa stores the access and
refresh tokens in the SQLite file with mode `0600`; Pi refreshes and persists
them automatically. The file is owner-readable plaintext, not an operating
system keychain, so it remains a personal-runtime proof rather than production
secret storage.

Configure the node with an xAI model after login:

```sh
export RENOA_PI_PROVIDER='xai'
export RENOA_PI_MODEL='grok-4.5'
```

## Run with OpenCode Go

Use the model ID and API key supplied by OpenCode Go:

```sh
export RENOA_RCP_ENDPOINT='wss://your-coordinator/connect'
export RENOA_RCP_DEVICE_ID='device-uuid-from-enrollment'
export RENOA_RCP_CREDENTIAL='credential-from-enrollment'
export RENOA_NODE_STATE='/absolute/path/to/pi-node.sqlite'
export RENOA_PI_AUTH_STORE='/absolute/path/to/pi-auth.sqlite'
export RENOA_PI_PROVIDER='opencode-go'
export RENOA_PI_MODEL='your-opencode-go-model-id'
export RENOA_PI_INSTRUCTIONS='You are a careful coding agent.'
export RENOA_PI_TARGET='workspace:renoa'
export OPENCODE_API_KEY='your-opencode-key'
# Optional, but these two values must be set together.
export RENOA_PI_WORKSPACE_ROOT='/absolute/path/to/workspace'
export RENOA_PI_WORKSPACE_ACCESS='read_write' # or 'read'
pnpm install --frozen-lockfile --ignore-scripts
pnpm run build
pnpm start
```

Pi's OpenCode Go provider chooses the correct OpenAI Completions, OpenAI
Responses, or Anthropic adapter for the selected model. Renoa does not duplicate
that provider routing. `RENOA_PI_PROVIDER` currently accepts `opencode-go` and
`xai`; another provider is added only after its real authentication and model
path are proven.

`RENOA_PI_PROVIDER`, `RENOA_PI_MODEL`, `RENOA_PI_AUTH_STORE`,
`RENOA_PI_INSTRUCTIONS`, and `RENOA_PI_TARGET` are required. The auth-store and
optional workspace paths must be absolute; a workspace also requires an
explicit access mode. Harness configuration stays local to the node and is
never sent over RCP. The command target must exactly match the configured
target. A target mismatch, parent traversal, or symlink leaving the root fails
closed.

The workspace check confines normal Pi file operations, but it is not a hostile
filesystem sandbox: another local process could race a path check by changing a
symlink. Bash remains disabled until Renoa has a real sandbox and approval
boundary.

This process currently hosts one Pi configuration. A future node-side registry
can bind different tasks to different harness configurations without changing
the RCP delivery shape.
