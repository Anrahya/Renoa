/**
 * Official OpenCode Go transports, verified 2026-08-25 against
 * https://dev.opencode.ai/docs/go/
 *
 * Models are advertised only when this table names a transport and the pinned
 * Pi catalog supplies limits, tool support, and reasoning metadata. Transport
 * is never inferred from a model id.
 */
export type OpenCodeTransport = "openai-completions" | "openai-responses" | "anthropic-messages";

export const OPENCODE_GO_BASE_URL = {
  "openai-completions": "https://opencode.ai/zen/go/v1",
  "openai-responses": "https://opencode.ai/zen/go/v1",
  "anthropic-messages": "https://opencode.ai/zen/go",
} as const;

export const OPENCODE_GO_TRANSPORTS: Readonly<Record<string, OpenCodeTransport>> = {
  "grok-4.5": "openai-responses",
  "gpt-5.6-luna": "openai-responses",
  "muse-spark-1.2-contributor": "openai-responses",
  "glm-5.1": "openai-completions",
  "glm-5.2": "openai-completions",
  "glm-5.3": "openai-completions",
  "kimi-k3": "openai-completions",
  "kimi-k2.6": "openai-completions",
  "kimi-k2.7-code": "openai-completions",
  "longcat-2.0": "openai-completions",
  "deepseek-v4-pro": "openai-completions",
  "deepseek-v4-flash": "openai-completions",
  "deepseek-v4-flash-vision-exp": "openai-completions",
  "mimo-v2.5": "openai-completions",
  "mimo-v2.5-pro": "openai-completions",
  hy3: "openai-completions",
  "ox-alpha-free": "openai-completions",
  "minimax-m3": "anthropic-messages",
  "minimax-m2.7": "anthropic-messages",
  "minimax-m2.5": "anthropic-messages",
  "qwen3.8-max": "anthropic-messages",
  "qwen3.7-max": "anthropic-messages",
  "qwen3.7-plus": "anthropic-messages",
  "qwen3.6-plus": "anthropic-messages",
};

export function opencodeGoTransport(modelId: string): OpenCodeTransport | undefined {
  return OPENCODE_GO_TRANSPORTS[modelId];
}
