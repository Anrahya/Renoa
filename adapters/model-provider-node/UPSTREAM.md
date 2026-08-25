# Pi upstream adaptation

Adapted from [Pi](https://github.com/earendil-works/pi) `packages/ai`.

- Source revision: `914cf1472e715297caa30db4b9535d534a9eb718` (tag `v0.84.2`)
- License: MIT
- Copyright (c) 2025 Mario Zechner

Full license text: `src/upstream/LICENSE-MIT`.

The Renoa-owned OpenCode Go catalog overlay also projects metadata from
[models.dev](https://github.com/anomalyco/models.dev):

- Source revision: `be4e8d624fe57e129ef4e6523f8d774946f29b81`
- Source path: `providers/opencode-go/models/ox-alpha-free.toml`
- License: MIT
- Copyright (c) 2025 models.dev

Full license text: `src/upstream/LICENSE-MODELS-DEV-MIT`.

Relative imports in adapted TypeScript use `.js` specifiers for this package’s NodeNext tsconfig. Protocol logic is otherwise Pi’s.

Catalog JSON files are gitignored in the Pi clone (`packages/ai/src/providers/data/`). They were copied from the published `@earendil-works/pi-ai@0.84.2` package (`dist/providers/data/`), which is the hydrated output of those source paths at the same version.

## Copied files

| Renoa path | Pi source path |
| --- | --- |
| `src/upstream/LICENSE-MIT` | `LICENSE` |
| `src/upstream/types.ts` | `packages/ai/src/types.ts` (pruned to xAI/OpenCode: three APIs; thinkingFormat `openai`/`deepseek`; session affinity `openai`/`openai-nosession`; JSON-schema constrained sampling only; no OpenRouter/Vercel/chat-template/grammar/deferred-tool/Claude-adaptive types; OAuth types live in `oauth-xai.ts`) |
| `src/upstream/thinking.ts` | `packages/ai/src/models.ts` (`calculateCost`, `getSupportedThinkingLevels`, `clampThinkingLevel`) |
| `src/upstream/event-stream.ts` | `packages/ai/src/utils/event-stream.ts` |
| `src/upstream/simple-options.ts` | `packages/ai/src/api/simple-options.ts` |
| `src/upstream/overflow.ts` | `packages/ai/src/utils/overflow.ts` |
| `src/upstream/error-body.ts` | `packages/ai/src/utils/error-body.ts` |
| `src/upstream/estimate.ts` | `packages/ai/src/utils/estimate.ts` |
| `src/upstream/headers.ts` | `packages/ai/src/utils/headers.ts` |
| `src/upstream/hash.ts` | `packages/ai/src/utils/hash.ts` |
| `src/upstream/json-parse.ts` | `packages/ai/src/utils/json-parse.ts` |
| `src/upstream/sanitize-unicode.ts` | `packages/ai/src/utils/sanitize-unicode.ts` |
| `src/upstream/diagnostics.ts` | `packages/ai/src/utils/diagnostics.ts` |
| `src/upstream/transform-messages.ts` | `packages/ai/src/api/transform-messages.ts` |
| `src/upstream/constrained-sampling.ts` | `packages/ai/src/api/constrained-sampling.ts` (JSON-schema strict helpers only; grammar tools not advertised) |
| `src/upstream/openai-prompt-cache.ts` | `packages/ai/src/api/openai-prompt-cache.ts` |
| `src/upstream/openai-chat-stream.ts` | `packages/ai/src/api/openai-completions.ts` (stream / streamSimple) |
| `src/upstream/openai-chat-client.ts` | `packages/ai/src/api/openai-completions.ts` (client) |
| `src/upstream/openai-chat-params.ts` | `packages/ai/src/api/openai-completions.ts` (params) |
| `src/upstream/openai-chat-messages.ts` | `packages/ai/src/api/openai-completions.ts` (messages) |
| `src/upstream/openai-chat-compat.ts` | `packages/ai/src/api/openai-completions.ts` (compat) |
| `src/upstream/openai-completions.ts` | re-export of stream + streamSimple + `OpenAICompletionsOptions` |
| `src/upstream/openai-responses.ts` | `packages/ai/src/api/openai-responses.ts` |
| `src/upstream/openai-responses-messages.ts` | `packages/ai/src/api/openai-responses-shared.ts` (messages) |
| `src/upstream/openai-responses-stream.ts` | `packages/ai/src/api/openai-responses-shared.ts` (stream) |
| `src/upstream/openai-responses-tools.ts` | `packages/ai/src/api/openai-responses-shared.ts` (tools) |
| `src/upstream/anthropic-sse.ts` | `packages/ai/src/api/anthropic-messages.ts` (SSE) |
| `src/upstream/anthropic-stream.ts` | `packages/ai/src/api/anthropic-messages.ts` (stream / streamSimple) |
| `src/upstream/anthropic-client.ts` | `packages/ai/src/api/anthropic-messages.ts` (client) |
| `src/upstream/anthropic-params.ts` | `packages/ai/src/api/anthropic-messages.ts` (params) |
| `src/upstream/anthropic-convert.ts` | `packages/ai/src/api/anthropic-messages.ts` (convert) |
| `src/upstream/anthropic-messages.ts` | re-export of stream + streamSimple + option types |
| `src/upstream/device-code.ts` | `packages/ai/src/auth/oauth/device-code.ts` |
| `src/upstream/oauth-xai.ts` | `packages/ai/src/auth/oauth/xai.ts` plus OAuth types from `packages/ai/src/auth/types.ts` |
| `src/upstream/catalogs/xai.json` | `packages/ai/src/providers/data/xai.json` |
| `src/upstream/catalogs/opencode-go.json` | `packages/ai/src/providers/data/opencode-go.json` |

## Not copied

- `packages/ai/src/utils/provider-retry.ts` — transports await each request once (`maxRetries: 0`). Renoa retry belongs in `src/retry.ts`.
- `packages/ai/src/utils/pi-user-agent.ts` — Anthropic client sends `Renoa/0.1`.
- `packages/ai/src/utils/deferred-tools.ts` — no advertised xAI or OpenCode model enables deferred tools or tool search. Anthropic conversion does not emit `tool_reference` or `defer_loading`.
- `packages/ai/src/api/lazy.ts` (`lazyStream`) — unused after prune; Renoa owns stream lifecycle.
- `packages/ai/src/utils/provider-env.ts` — unused after prune; providers take explicit credentials.
- `packages/ai/src/utils/abort.ts` — unused after prune; callers pass abort signals directly.
- Telemetry package, typebox, image APIs, and other provider APIs.
- GitHub Copilot header injection and Kimi Coding User-Agent overrides. Those providers are not advertised.
- OpenRouter, Vercel, Together, Baseten, Z.ai, chat-template, and string-thinking Chat Completions policy. Those providers are not advertised.
- OpenAI grammar / custom tools, Claude adaptive-thinking / tool-search defaults, Vertex client injection, and `PI_CACHE_RETENTION`. No advertised catalog entry sets those fields; cache retention is the explicit request field or `"short"`.
- Kimi `deferredToolsMode` Chat Completions system-tool messages. No advertised OpenCode or xAI model sets that mode.
- Claude Code OAuth identity (`You are Claude Code`, `claude-cli` user-agent, `sk-ant-oat` client). Anthropic Messages is used only for OpenCode Go models.
- Chat Completions `thinkingFormat: "qwen"` / `enable_thinking` — OpenCode `qwen3.6-plus` (and sibling Qwen ids) are remapped to Anthropic Messages via `OPENCODE_GO_TRANSPORTS`, so that Chat Completions branch is unreachable for advertised models.

## Retained unusual branches

These remain because advertised xAI or OpenCode Go models still require them:

- `thinkingFormat: "deepseek"` — OpenCode `deepseek-v4-*` and `kimi-k2.6` send `{ thinking: { type } }` on Chat Completions.
- `maxTokensField: "max_tokens"` — OpenCode Chat Completions models (GLM, Kimi, MiniMax, and others) still send `max_tokens` rather than `max_completion_tokens`.
- `sessionAffinityFormat: "openai-nosession"` — OpenCode Responses `grok-4.5` and `gpt-5.6-luna` set this in the pinned catalog. Other Responses models keep the OpenAI default.
- xAI/OpenCode `supportsStore: false` and `supportsDeveloperRole: false` — both remaining providers reject OpenAI stored responses and the developer role.
- Default OpenAI `reasoning_effort` mapping — Grok 4.6 overrides `supportsReasoningEffort` and maps kernel levels onto Chat Completions `reasoning_effort`.
- JSON-schema strict tool conversion — OpenAI Chat Completions and Responses still convert advertised function tools through `getJsonSchemaToolParameters` / `resolveJsonSchemaStrictSampling`.
