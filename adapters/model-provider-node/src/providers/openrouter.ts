/**
 * OpenRouter's endpoints, as Pi `packages/ai/src/providers/openrouter.ts`
 * (v1.5.0) configures them: Chat Completions under `/api/v1`, and Anthropic
 * Messages under `/api` for the Claude entries in Pi's catalog.
 */
export const OPENROUTER_BASE_URL = {
  "openai-completions": "https://openrouter.ai/api/v1",
  "anthropic-messages": "https://openrouter.ai/api",
} as const;
