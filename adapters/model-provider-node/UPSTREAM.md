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
- For a model whose effort Pi sets with mid-conversation markers (Claude Haiku
  and Sonnet 5.5), each replayed reply carries the session's reasoning level
  (`withReplayedEffort`), so Pi puts back the marker it was sent with and the
  reply's thinking stays valid. Pi then also sends
  `block_binding.prefix_mismatch_behavior: drop_block`, so a block from before
  a compaction, a changed system prompt, or a changed level is dropped instead
  of refusing the turn.

Compat switches Renoa's contract cannot carry are removed from every pinned
catalog entry (`UNCARRIED_COMPAT_FIELDS` in `src/catalog.ts`): mid-conversation
system messages and tool changes, additional tools, and server-side fallback
models. A request has one system prompt, one tool list, and an answer only from
the bound model.

Pi reports tool-call arguments it cannot parse as `{}` with `argumentsError`;
when the reply asks to run that call, the turn fails as a protocol error
instead (`src/wire-request.ts`). An Anthropic key containing `sk-ant-oat`, which
Pi would send as Claude Code, is refused at enrolment and before any request.

Pi 1.5.0 moved OpenCode Go to `/inference/go/openai/v1` and
`/inference/go/anthropic`; Renoa still pins the earlier routes in
`src/providers/opencode-go.ts`, which serve today. Anthropic Messages requests go
to the SDK's `/v1/messages?beta=true` route.

What Renoa keeps for itself: the wire contract and its mapping
(`src/wire-request.ts`), the SQLite credential store, the advertised provider
list and trusted base URLs (`src/catalog.ts`), OpenCode Go's documented
transport table (`src/providers/opencode-go.ts`), live discovery for OpenCode
Go and OpenRouter (`src/opencode-catalog.ts`, `src/openrouter-catalog.ts`, and
their shared cache in `src/live-catalog.ts`), and failure classification
(`src/errors.ts`). OpenRouter prices are converted with Pi's `openRouterCost`.

Pi's own model registry (`createModels`), credential store, telemetry, and
image and classifier APIs are not used. The build scripts of Pi's
`@google/genai` dependency and its `protobufjs` are refused in
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

## OpenRouter

OpenRouter discovery reads its public model list
(`https://openrouter.ai/api/v1/models`, fields verified 2026-11-12) and adds the
chat models Pi does not pin that are served on `/api/v1/chat/completions`, read
and write text, accept tools, and state a context window and output cap. An
entry Renoa cannot read is skipped without discarding the rest of the list. Reasoning levels come from each model's
`reasoning.supported_efforts`, without `off` when `reasoning.mandatory` is set.
No compat block is written: Pi derives OpenRouter's reasoning object, roles,
and session affinity from the provider and base URL. Per-model flags that Pi's
bundled entries carry, such as strict tool schemas, are not inferred.

## Google Vertex AI

Pi's Vertex transport and catalog are used as published. Two things are
Renoa's own (`src/providers/google-vertex.ts`): a Vertex API key bound to a
service account reaches a model only under its project's path, so enrolment
stores the project and location with the key and each request is routed there;
and Gemini 4 Hydrogen, which Pi 1.5.2 does not list, is pinned by Renoa in the
shape of Pi's Gemini 4 Argon entry until the pinned Pi package lists it. Pi's
transport refuses a custom fetch, so Vertex failures are classified as possibly
dispatched.
