/**
 * Official OpenCode Go transports, verified 2026-10-29 against
 * https://opencode.ai/docs/go/
 *
 * This table records the documented transport of every current OpenCode Go
 * model. Bundled catalog entries are advertised only when listed here; live
 * catalog entries otherwise use a known models.dev SDK transport. A model name
 * or id is never used to guess its transport. Models served only through the
 * Google Generative AI API (for example `gemini-4-argon`) are absent because
 * this adapter does not carry that API.
 */
export type OpenCodeTransport = "openai-completions" | "openai-responses" | "anthropic-messages";

export const OPENCODE_GO_BASE_URL = {
  "openai-completions": "https://opencode.ai/inference/go/v1",
  "openai-responses": "https://opencode.ai/inference/go/v1",
  "anthropic-messages": "https://opencode.ai/inference/go",
} as const;

export const OPENCODE_GO_TRANSPORTS: Readonly<Record<string, OpenCodeTransport>> = {
  "grok-4.7": "openai-responses",
  "gpt-6-luna": "openai-responses",
  "gpt-5.6-luna": "openai-responses",
  "muse-spark-1.3-contributor": "openai-responses",
  "muse-spark-1.2-contributor": "openai-responses",
  "claude-haiku-5-5": "anthropic-messages",
  "minimax-m3": "anthropic-messages",
  "qwen3.8-max": "anthropic-messages",
  "qwen3.8-flash": "anthropic-messages",
  "qwen3.7-plus": "anthropic-messages",
  "glm-5.3-flash": "openai-completions",
  "glm-5.3": "openai-completions",
  "glm-5.2": "openai-completions",
  "kimi-k3.1-pro": "openai-completions",
  "kimi-k3": "openai-completions",
  "kimi-k2.7-code": "openai-completions",
  "longcat-2.0": "openai-completions",
  "deepseek-v4.1-flash": "openai-completions",
  "deepseek-v4-pro": "openai-completions",
  "mimo-v2.6-flash": "openai-completions",
  "mimo-v2.6-pro": "openai-completions",
  "mimo-v2.5": "openai-completions",
  "mimo-v2.5-pro": "openai-completions",
  "minimax-m3.1-flash": "openai-completions",
  "hy4-preview": "openai-completions",
  hy3: "openai-completions",
  "step-5-preview": "openai-completions",
  "beam-free": "openai-completions",
  "iceman-contributor-free": "openai-completions",
};

export function opencodeGoTransport(
  modelId: string,
  npmPackage?: string,
): OpenCodeTransport | undefined {
  const documented = OPENCODE_GO_TRANSPORTS[modelId];
  if (documented !== undefined) {
    return documented;
  }
  switch (npmPackage) {
    case "@ai-sdk/openai-compatible":
      return "openai-completions";
    case "@ai-sdk/openai":
      return "openai-responses";
    case "@ai-sdk/anthropic":
      return "anthropic-messages";
    default:
      return undefined;
  }
}
