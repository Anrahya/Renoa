# Renoa local host

`renoa-local` is the first real Host for `renoa-kernel`. It resolves one local
coding runtime from:

- Renoa Alpha's versioned coding behavior and workspace `AGENTS.md` rules;
- Pi AI model routing through a small Node process adapter; and
- Renoa's durable model/tool loop and compaction strategy; and
- local `read_file`, `edit_file`, `write_file`, `bash`, `grep`, and `find`
  tools.

Provider credentials and tool implementations stay outside the kernel.
The host is intentionally all-allowed: registering a tool makes it available.
Paths are confined to the configured workspace, but `bash` is unrestricted and
this is not a sandbox for untrusted work.

Model-visible output is bounded: file reads are paginated, `grep` returns at
most 100 matches, `find` returns at most 1,000 paths, and process output keeps
the final 50 KiB or 2,000 lines. Search uses the resolved `rg` executable with
configuration disabled, deterministic ordering, ignore-file handling, and
workspace-relative results. Hidden paths, including `.git`, are intentionally
skipped by `grep` and `find`; unrestricted `bash` remains the explicit escape
hatch. `rg` is therefore a required local executable.

The architecture and deliberately open permission decisions are recorded in
[`docs/renoa-host-v0.md`](../../docs/renoa-host-v0.md). The ACP adapter still
uses the legacy harness path until the kernel path has the transient observation
boundary required by a real surface.

Build the Pi bridge first:

```sh
pnpm --dir nodes/pi install --frozen-lockfile --ignore-scripts
pnpm --dir nodes/pi build
```

Enroll provider credentials into an owner-only SQLite file before starting the
host. SuperGrok uses `pnpm --dir nodes/pi auth:xai`. OpenCode Go uses the
non-echoing piped-key flow documented in
[`nodes/pi/README.md`](../../nodes/pi/README.md#run-with-opencode-go).

Then configure one Pi model and run a turn:

```sh
export RENOA_PI_BRIDGE='/absolute/path/to/nodes/pi/dist/src/model-bridge-main.js'
export RENOA_PI_AUTH_STORE='/absolute/path/to/pi-auth.sqlite'
export RENOA_PI_PROVIDER='xai' # or opencode-go
export RENOA_PI_MODEL='grok-4.6'
export RENOA_PI_REASONING='high' # optional: off|minimal|low|medium|high|xhigh|max

cargo run -p renoa-local -- \
  /absolute/path/to/kernel.sqlite \
  /absolute/path/to/workspace \
  new \
  'Read the project, make the requested change, and run its build.'
```

The command prints the stable session ID. Pass that ID instead of `new` to add
the next turn to the same durable conversation. `Ctrl-C` requests ordered
kernel cancellation and waits for active model or process work to stop.

The normal runner always uses Renoa Alpha. Change `RENOA_PI_MODEL` or
`RENOA_PI_REASONING` before the next command to change that operation's model
behavior without replacing the session or its history. An active operation
keeps the exact runtime already frozen by the kernel.

Alpha loads `AGENTS.md` from the canonical workspace root. The file must be
UTF-8, remain inside the workspace after symlink resolution, and fit within 32
KiB. Oversized rules fail clearly instead of entering the context partially.
The full profile contract and research record are in
[`docs/renoa-alpha-v1.md`](../../docs/renoa-alpha-v1.md).

At startup the Host resolves the selected model's context and output limits.
Authentication is resolved when the first model stream starts. A credential
rejection proven to happen before inference fails the operation clearly and
leaves the session usable; a failure after output starts remains uncertain. A
packaged Pi model stays package-pinned; an xAI model
absent from that package can be resolved from Pi's official live catalog. Renoa
validates the provider, API, and xAI endpoint, then pins the exact model record
and includes its SHA-256 identity in the runtime revision. The kernel freezes
that revision with the loop, context configuration, instructions, and
workspace-bound tools before an operation runs. An interrupted operation
therefore fails closed instead of silently changing behavior after restart.

The Host caps model output at 32,768 tokens and enables durable context
checkpoints with explicit headroom. The local token estimate is deliberately
conservative but not treated as exact. An explicit pre-inference provider
context rejection is persisted and forces compaction without replaying the
rejected request.
