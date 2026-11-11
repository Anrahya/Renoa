# Upstream use

## Pi

The provider protocols (Anthropic Messages, OpenAI Chat Completions, OpenAI
Responses), the bundled model catalogs, and the xAI OAuth flow come from Pi's
published model package, imported as a dependency rather than copied.

- Package: [`@earendil-works/pi-ai`](https://github.com/earendil-works/pi) `1.5.0`,
  pinned exactly in `package.json`
- License: MIT, Copyright (c) 2025 Mario Zechner; the license ships inside the
  installed package
- A new model or provider arrives by bumping the version. `test/pi-protocol.test.ts`
  pins the protocol behaviour Renoa depends on, so a bump that changes it fails
  there.

What Renoa sets on every request (`src/stream.ts`):

- `maxRetries: 0`: Pi's client retries are off; Renoa's retry lives in `src/retry.ts`.
- `identity`: `User-Agent: renoa/0.1.0` and no attribution headers; no Pi
  identity is sent.
- `timeoutMs`: 30 minutes, because the Host owns the invocation deadline.

What Renoa keeps for itself: the wire contract and its mapping
(`src/wire-request.ts`), the SQLite credential store, the advertised provider
list and trusted base URLs (`src/catalog.ts`), OpenCode Go's documented
transport table (`src/providers/opencode-go.ts`) and live discovery
(`src/opencode-catalog.ts`), and failure classification (`src/errors.ts`).

Pi's own model registry (`createModels`), credential store, telemetry, and
image and classifier APIs are not used. The build scripts of Pi's optional
`@google/genai` and `protobufjs` dependencies are refused in
`pnpm-workspace.yaml`; Renoa advertises no provider that uses them.

## models.dev

Live OpenCode Go discovery projects metadata from the MIT-licensed
[models.dev](https://github.com/anomalyco/models.dev) schema at
`https://models.dev/api.json`, verified against models.dev revision
`826d9fe918d9d208791314dd6892f7327c1de5b2` (Copyright (c) 2025 models.dev; full
license text: `LICENSE-MODELS-DEV-MIT`). Availability comes from OpenCode's
official Go model endpoint (`https://opencode.ai/inference/go/v1/models`), and
`OPENCODE_GO_TRANSPORTS` records the transport of each model in OpenCode's Go
documentation (`https://opencode.ai/docs/go/`), both verified 2026-10-29. No
OpenCode source code is copied.
