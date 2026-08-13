# Renoa local host

`renoa-local` is the first real coding host for `renoa-harness`. It binds one
durable harness session to:

- Pi AI model routing through a small Node process adapter; and
- local `read_file`, `edit_file`, `write_file`, and `bash` tools.

Provider credentials and tool implementations stay outside the harness core.
The host is intentionally all-allowed: registering a tool makes it available.
Paths are confined to the configured workspace, but `bash` is unrestricted and
this is not a sandbox for untrusted work.

Build the Pi bridge first:

```sh
pnpm --dir nodes/pi install --frozen-lockfile --ignore-scripts
pnpm --dir nodes/pi build
```

Then configure one Pi model and run a turn:

```sh
export RENOA_PI_BRIDGE='/absolute/path/to/nodes/pi/dist/src/model-bridge-main.js'
export RENOA_PI_AUTH_STORE='/absolute/path/to/pi-auth.sqlite'
export RENOA_PI_PROVIDER='xai' # or opencode-go
export RENOA_PI_MODEL='grok-4.5'
export RENOA_PI_INSTRUCTIONS='You are a careful coding agent.'

cargo run -p renoa-local -- \
  /absolute/path/to/harness.sqlite \
  /absolute/path/to/workspace \
  new \
  'Read the project, make the requested change, and run its build.'
```

The command prints the stable session ID. Pass that ID instead of `new` to add
the next turn to the same durable conversation. `Ctrl-C` requests ordered
harness cancellation and waits for active model or process work to stop.

At startup the Pi bridge resolves the selected model's context and output
limits from Pi's model catalog and verifies authentication. The host caps model
output at 32,768 tokens and enables durable context checkpoints with explicit
headroom; no provider table is compiled into the Rust harness. The local token
estimate is deliberately conservative but not treated as exact. An explicit
pre-inference provider context rejection is persisted and forces compaction
without replaying the rejected request.
