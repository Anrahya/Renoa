/**
 * Official OpenCode Go transports, verified 2026-08-25 against
 * https://dev.opencode.ai/docs/go/
 *
 * This table records model-specific corrections from OpenCode's documentation.
 * Live catalog entries otherwise use a known models.dev SDK transport. A model
 * name or id is never used to guess its transport.
 */
export type OpenCodeTransport = "openai-completions" | "openai-responses" | "anthropic-messages";

export const OPENCODE_GO_BASE_URL = {
  "openai-completions": "https://opencode.ai/zen/go/v1",
  "openai-responses": "https://opencode.ai/zen/go/v1",
  "anthropic-messages": "https://opencode.ai/zen/go",
} as const;

/**
 * Current OpenCode Go models absent from the pinned Pi catalog. Metadata is a
 * Renoa-supported projection of models.dev commit
 * be4e8d624fe57e129ef4e6523f8d774946f29b81 (MIT): video input is omitted
 * because Renoa's provider-neutral contract currently carries text and images.
 */
export const OPENCODE_GO_CATALOG_ADDITIONS: readonly Record<string, unknown>[] = [
  {
    id: "ox-alpha-free",
    name: "Ox Alpha Free (Unlimited)",
    api: "openai-completions",
    provider: "opencode-go",
    baseUrl: OPENCODE_GO_BASE_URL["openai-completions"],
    reasoning: true,
    input: ["text", "image"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    compat: {
      supportsStore: false,
      supportsDeveloperRole: false,
      maxTokensField: "max_tokens",
    },
    contextWindow: 1_000_000,
    maxTokens: 131_072,
    thinkingLevelMap: {
      off: null,
      minimal: null,
      low: "low",
      medium: null,
      high: "high",
      xhigh: null,
      max: "max",
    },
  },
];

export const OPENCODE_GO_TRANSPORTS: Readonly<Record<string, OpenCodeTransport>> = {
  "grok-4.5": "openai-responses",
  "grok-4.6": "openai-responses",
  "gpt-5.6-luna": "openai-responses",
  "muse-spark-1.2-contributor": "openai-responses",
  "glm-5.1": "openai-completions",
  "glm-5.2": "openai-completions",
  "glm-5.3": "openai-completions",
  "glm-5.3-flash": "openai-completions",
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
