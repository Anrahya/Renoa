# Renoa local host

`renoa-local` is the first real Host for `renoa-kernel`. It resolves one local
coding runtime from:

- Renoa Alpha's versioned coding behavior and workspace `AGENTS.md` rules;
- the `@renoa/model-provider` process adapter for xAI and OpenCode Go; and
- Renoa's durable model/tool loop and compaction strategy;
- local `read_file`, `edit_file`, `write_file`, `bash`, `grep`, and `find`
  tools; and
- three fixed extension-registry tools over durable MCP integration,
  connection, catalog, and Alpha-attachment records.

Provider credentials and tool implementations stay outside the kernel.
The host is intentionally all-allowed. Attaching a connection makes its tools
searchable, but no external schema is advertised automatically. Alpha uses
`tool_search`, `tool_load`, and `tool_execute`; those bindings read committed
Host state on each call, so Waku and Alpha do not restart after a catalog
change. Paths are confined to the configured workspace, but `bash` is
unrestricted and this is not a sandbox for untrusted work.

Model-visible output is bounded: file reads are paginated, `grep` returns at
most 100 matches, `find` returns at most 1,000 paths, and process output keeps
the final 50 KiB or 2,000 lines. Search uses the resolved `rg` executable with
configuration disabled, deterministic ordering, ignore-file handling, and
workspace-relative results. Hidden paths, including `.git`, are intentionally
skipped by `grep` and `find`; unrestricted `bash` remains the explicit escape
hatch. `rg` is therefore a required local executable.

Every built-in tool has a total deadline. Read, edit, write, grep, and find use
120 seconds. Bash defaults to 120 seconds and accepts 1 through 1,800 seconds
per call. Cancellation and deadlines wait until owned work is stopped. Bash,
ripgrep, and model bridge processes run in isolated process groups so their
descendants cannot outlive the call. File writes and edits use synced atomic
replacement; edits reject a concurrent content change.

The architecture and deliberately open permission decisions are recorded in
[`docs/renoa-host-v0.md`](../../docs/renoa-host-v0.md). `LocalHost` and
`AlphaSession` are the complete boundary used by ACP; `LocalSession` is the
lower kernel command boundary also used by the headless diagnostic runner.
Live ACP updates come from a presentation-only event observer in the model and
tool adapters; the kernel remains the sole durable execution owner.

The Host data root contains `host.sqlite3` for MCP catalog and profile-attachment
state, non-secret OAuth phases and terminal receipts, and `oauth-locks/` for
per-connection refresh coordination, plus `sessions/<session-id>/`. OAuth credential bundles live in
the desktop Secret Service, not this data root; the current desktop flow
requires `secret-tool` and `xdg-open`. Each session uses `session.json` for
identity, `runtime.jsonl` for acknowledged provider/model/reasoning choices,
`kernel.sqlite3` for recovery truth, and `trace.sqlite3` for the ordered
diagnostic timeline. Token/cache usage, exact provider payloads, stream chunks,
durations, and tool diagnostics live only in the trace database.

Build the model-provider adapter first:

```sh
pnpm --dir adapters/model-provider-node install --frozen-lockfile --ignore-scripts
pnpm --dir adapters/model-provider-node build
```

Enroll provider credentials into an owner-only SQLite file before starting the
host. SuperGrok uses `pnpm --dir adapters/model-provider-node auth:xai`.
OpenCode Go uses the non-echoing piped-key flow:

```sh
printf '%s' "$OPENCODE_API_KEY" | pnpm --dir adapters/model-provider-node auth:opencode-go
```

Existing `~/.config/renoa/pi-auth.sqlite` files remain the credential store.
The architecture is recorded in
[`docs/model-provider-v0.md`](../../docs/model-provider-v0.md).

Then configure one model and run a turn:

```sh
export RENOA_MODEL_BRIDGE='/absolute/path/to/adapters/model-provider-node/dist/src/main.js'
export RENOA_MODEL_AUTH_STORE='/absolute/path/to/pi-auth.sqlite'
export RENOA_MODEL_PROVIDER='xai' # or opencode-go
export RENOA_MODEL='grok-4.6'
export RENOA_MODEL_REASONING='high' # optional: off|minimal|low|medium|high|xhigh|max

cargo run -p renoa-local -- \
  /absolute/path/to/kernel.sqlite \
  /absolute/path/to/workspace \
  new \
  'Read the project, make the requested change, and run its build.'
```

The command prints the stable session ID. Pass that ID instead of `new` to add
the next turn to the same durable conversation. `Ctrl-C` requests ordered
kernel cancellation and waits for active model or process work to stop.

The normal runner always uses Renoa Alpha. Change `RENOA_MODEL` or
`RENOA_MODEL_REASONING` before the next command to change that operation's model
behavior without replacing the session or its history. An active operation
keeps the exact runtime already frozen by the kernel.

Alpha loads `AGENTS.md` from the canonical workspace root before every new
turn. The file must be
UTF-8, remain inside the workspace after symlink resolution, and fit within 32
KiB. Oversized rules fail clearly instead of entering the context partially.
The full profile contract and research record are in
[`docs/renoa-alpha-v1.md`](../../docs/renoa-alpha-v1.md).

Before each newly admitted turn the Host resolves the selected model's context,
output limits, and current project instructions. Session creation and model
selection changes also validate the runtime before acknowledging success.
Authentication is resolved when the first model stream starts. A credential
rejection proven to happen before inference fails the operation clearly and
leaves the session usable; a failure after output starts remains uncertain. The
adapter advertises the pinned xAI and OpenCode Go catalogs only. Renoa
validates the provider, API, and endpoint, then pins the exact model record
and includes its SHA-256 identity in the runtime revision. The kernel freezes
that revision with the loop, context configuration, instructions, and
workspace-bound tools before an operation runs. An interrupted operation
therefore fails closed instead of silently changing behavior after restart.

The Host caps model output at 32,768 tokens and enables durable context
checkpoints with explicit headroom. The local token estimate is deliberately
conservative but not treated as exact. An explicit pre-inference provider
context rejection is persisted and forces compaction without replaying the
rejected request.
