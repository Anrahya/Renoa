# Pi upstream adaptation

Adapted from [Pi](https://github.com/earendil-works/pi) `packages/ai`.

- Source revision: `4a080252f06caf63a525d7873c5d94d3c23887e0` (tag `v1.4.0`)
- Previous revision: `914cf1472e715297caa30db4b9535d534a9eb718` (tag `v0.84.2`)
- License: MIT
- Copyright (c) 2025 Mario Zechner

Full license text: `src/upstream/LICENSE-MIT` (unchanged at `v1.4.0`).

Live OpenCode Go discovery projects metadata from the MIT-licensed
[models.dev](https://github.com/anomalyco/models.dev) schema at
`https://models.dev/api.json`, verified against models.dev revision
`826d9fe918d9d208791314dd6892f7327c1de5b2` (Copyright (c) 2025 models.dev; full
license text: `src/upstream/LICENSE-MODELS-DEV-MIT`). Availability comes from
OpenCode's official Go model endpoint (`https://opencode.ai/inference/go/v1/models`),
and `OPENCODE_GO_TRANSPORTS` records the transport of each model in OpenCode's Go
documentation (`https://opencode.ai/docs/go/`), both verified 2026-10-29. No
OpenCode source code is copied.

Relative imports in adapted TypeScript use `.js` specifiers for this package’s NodeNext tsconfig. Protocol logic is otherwise Pi’s.

Catalog JSON files are gitignored in the Pi clone (`packages/ai/src/providers/data/`). They were copied unmodified from the published `@earendil-works/pi-ai@1.4.0` package (`dist/providers/data/`), which is the hydrated output of those source paths at the same version. Their keys use Pi data schema 6 (`chat:<id>`), and entries carry `type`. `src/catalog.ts` advertises only `chat` entries on a retained API and removes the unconsumed `type` and `inputLimits` fields before an entry becomes a model binding.

## Copied files

| Renoa path | Pi source path |
| --- | --- |
| `src/upstream/LICENSE-MIT` | `LICENSE` |
| `src/upstream/types.ts` | `packages/ai/src/types.ts` (pruned to xAI/OpenCode: three APIs; thinkingFormat `openai`/`deepseek`; session affinity `openai`/`openai-nosession`; JSON-schema constrained sampling only; Anthropic `forceAdaptiveThinking`; no OpenRouter/Vercel/chat-template/grammar/deferred-tool/transcript/system-message/image-model/classifier types; OAuth types live in `oauth-xai.ts`) |
| `src/upstream/thinking.ts` | `packages/ai/src/models.ts` (`calculateCost`, `getSupportedThinkingLevels`, `clampThinkingLevel`) |
| `src/upstream/event-stream.ts` | `packages/ai/src/utils/event-stream.ts` (without `durationMs` timing) |
| `src/upstream/simple-options.ts` | `packages/ai/src/api/simple-options.ts` |
| `src/upstream/overflow.ts` | `packages/ai/src/utils/overflow.ts` |
| `src/upstream/error-body.ts` | `packages/ai/src/utils/error-body.ts` |
| `src/upstream/estimate.ts` | `packages/ai/src/utils/estimate.ts` (takes `Context`, not `TranscriptContext`) |
| `src/upstream/headers.ts` | `packages/ai/src/utils/headers.ts` |
| `src/upstream/hash.ts` | `packages/ai/src/utils/hash.ts` |
| `src/upstream/json-parse.ts` | `packages/ai/src/utils/json-parse.ts` (without `parseToolCallArguments`) |
| `src/upstream/sanitize-unicode.ts` | `packages/ai/src/utils/sanitize-unicode.ts` |
| `src/upstream/diagnostics.ts` | `packages/ai/src/utils/diagnostics.ts` |
| `src/upstream/transform-messages.ts` | `packages/ai/src/api/transform-messages.ts` (no system-message handling) |
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

- `packages/ai/src/utils/provider-retry.ts` and `utils/retry.ts` — transports await each request once (`maxRetries: 0`). Renoa retry belongs in `src/retry.ts`.
- `packages/ai/src/utils/identity.ts` (client identity, `User-Agent`, attribution headers) and the removed `pi-user-agent.ts` — no Pi identity is sent. OpenCode Go requests send `user-agent: renoa/0.1.0` (`src/stream.ts`); xAI requests keep the OpenAI SDK default.
- `packages/ai/src/utils/transcript.ts`, `utils/text.ts`, and mid-conversation system messages and tool changes (`supportsMidConvoSystemMessages`, `supportsMidConvoToolAdditions`, `supportsMidConvoToolChanges`, `supportsAdditionalTools`, Anthropic `inline-tools-2026-09-15`, `tool_addition` / `tool_removal`). Renoa's contract carries one system prompt and one tool list per request; catalog entries that set these flags are sent through the single-prompt path, which ignores them.
- Anthropic SDK upgrade to the beta Messages API (`betas` parameter), server-side model fallbacks (`allowedFallbackModels`), mid-conversation effort (`supportsMidConvoEffort`), strict-tool `prefer` fallback, input-transformation diagnostics, and workload identity federation. No advertised model sets those fields, and Renoa tools carry no constrained-sampling configuration.
- OpenAI Responses `configuration_update` effort items, Sign in with ChatGPT handling, `supportsMaxOutputTokens`, and explicit prompt-cache TTL options. No advertised model sets those fields.
- `parseToolCallArguments` / `ToolCall.argumentsError` — reporting malformed final tool-call arguments needs a field in Renoa's provider-neutral contract. Final arguments still come from `parseStreamingJson`.
- `AssistantMessage.durationMs`, `providerThinkingLevel`, `onProviderStreamEvent`, and `samplingParamsByThinkingLevel` — not consumed by the runtime.
- OpenRouter `reasoning_details` replay changes in Chat Completions. Renoa keeps the `v0.84.2` encrypted-detail handling; no advertised model is known to emit `reasoning_details`.
- `thinkingTokenBudgetField`, `vllmPriority`, chat-template budgets — no advertised catalog entry sets those fields.
- Google Generative AI API (`packages/ai/src/api/google-generative-ai.ts`, `google-shared.ts`). OpenCode Go serves its Gemini models (for example `gemini-4-argon-contributor` in the pinned catalog) only through that API, so they are filtered out of the advertised catalog and absent from `OPENCODE_GO_TRANSPORTS`.
- Image and classifier models (`type: "image"` / `"classifier"`) and their APIs; only `chat` entries are advertised. `inputLimits` image-resize metadata is removed because Renoa does not resize images.
- Pinned OpenCode Go entries that OpenCode's Go documentation no longer lists (`longcat-2.5-preview-free`, `minimax-m3.1-flash-free`) are not advertised.
- `packages/ai/src/utils/deferred-tools.ts` (removed upstream), `api/lazy.ts`, `utils/provider-env.ts`, `utils/abort.ts` — unused after prune.
- Telemetry package, typebox, image APIs, and other provider APIs.
- GitHub Copilot header injection and Kimi Coding User-Agent overrides. Those providers are not advertised.
- OpenRouter, Vercel, Together, Baseten, Z.ai, chat-template, and string-thinking Chat Completions policy. Those providers are not advertised.
- OpenAI grammar / custom tools, Claude tool-search defaults, Vertex client injection, and `PI_CACHE_RETENTION`. No advertised catalog entry sets those fields; cache retention is the explicit request field or `"short"`.
- Claude Code OAuth identity (`You are Claude Code`, `claude-cli` user-agent, `sk-ant-oat` client). Anthropic Messages is used only for OpenCode Go models with an API key.
- Chat Completions `thinkingFormat: "qwen"` / `enable_thinking` — no advertised model sets it; OpenCode Go Qwen models use Anthropic Messages.
- xAI over Chat Completions. Pi `v1.4.0` serves every xAI model through Responses, so `src/catalog.ts` accepts only `openai-responses` for xAI.

## Retained unusual branches

These remain because advertised xAI or OpenCode Go models still require them:

- `thinkingFormat: "deepseek"` — OpenCode `deepseek-v4-pro` and `deepseek-v4.1-flash` send `{ thinking: { type } }` on Chat Completions.
- `maxTokensField: "max_tokens"` — OpenCode Chat Completions models (GLM, Kimi, MiMo, DeepSeek, and others) still send `max_tokens` rather than `max_completion_tokens`.
- `sessionAffinityFormat: "openai-nosession"` — OpenCode Responses `grok-4.7`, `gpt-5.6-luna`, `gpt-6-luna`, and the Muse Spark models set this in the pinned catalog. Other Responses models keep the OpenAI default.
- xAI/OpenCode `supportsStore: false` and `supportsDeveloperRole: false` — both remaining providers reject OpenAI stored responses and the developer role.
- Default OpenAI `reasoning_effort` mapping — OpenCode Chat Completions models without a `thinkingFormat` (for example GLM-5.3 and Kimi K3) send kernel levels as `reasoning_effort`.
- xAI Responses `include: ["reasoning.encrypted_content"]` — xAI returns encrypted reasoning for replay with `store: false`.
- Anthropic `forceAdaptiveThinking` — OpenCode `claude-haiku-5-5` sends `thinking: { type: "adaptive" }` with `output_config.effort` and no interleaved-thinking beta; `off` sends `thinking: { type: "disabled" }`.
- Anthropic `allowEmptySignature` — OpenCode `qwen3.8-flash` returns empty thinking signatures.
- JSON-schema strict tool conversion — OpenAI Chat Completions and Responses still convert advertised function tools through `getJsonSchemaToolParameters` / `resolveJsonSchemaStrictSampling`. Chat Completions sends `strict` only when `compat.supportsStrictMode` is set; the Pi catalog sets it for OpenCode Go models.
