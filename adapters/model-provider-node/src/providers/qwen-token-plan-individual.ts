/**
 * Qwen Token Plan Individual's international gateway, as Pi
 * `packages/ai/src/providers/qwen-token-plan-individual.ts` (v1.6.1) configures
 * it.
 */
export const QWEN_TOKEN_PLAN_INDIVIDUAL_BASE_URL =
  "https://token-plan.maas.qwencloudapi.com/compatible-mode/v1";

/**
 * Qwen4 Max, which Pi 1.6.1 lists in `qwen-token-plan` but not yet in
 * `qwen-token-plan-individual`. Shape mirrors Pi's `qwen-token-plan` entry
 * verbatim with this provider id. Remove this entry once the pinned Pi package
 * lists `qwen4-max` in `qwen-token-plan-individual`: Pi's own entry then takes
 * its place.
 */
export const QWEN4_MAX = {
  id: "qwen4-max",
  name: "Qwen4 Max",
  api: "openai-completions",
  provider: "qwen-token-plan-individual",
  baseUrl: QWEN_TOKEN_PLAN_INDIVIDUAL_BASE_URL,
  reasoning: true,
  input: ["text", "image"],
  cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
  contextWindow: 1_000_000,
  maxTokens: 262_144,
  compat: {
    supportsStrictMode: true,
    thinkingFormat: "qwen",
    supportsStore: false,
    supportsReasoningEffort: true,
  },
  thinkingLevelMap: {
    off: "none",
    minimal: null,
    low: "low",
    medium: null,
    high: "high",
    xhigh: "xhigh",
    max: null,
  },
  type: "chat",
} as const;
