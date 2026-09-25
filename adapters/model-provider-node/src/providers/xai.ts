import { xaiOAuth, type OAuthCredential } from "../upstream/oauth-xai.js";
import type { Credential } from "../credentials.js";

export const XAI_BASE_URL = "https://api.x.ai/v1";

export const XAI_CATALOG_ADDITIONS: readonly Record<string, unknown>[] = [
  {
    id: "grok-4.7",
    name: "Grok 4.7",
    api: "openai-completions",
    provider: "xai",
    baseUrl: XAI_BASE_URL,
    reasoning: true,
    input: ["text", "image"],
    cost: { input: 2, output: 6, cacheRead: 0.5, cacheWrite: 0 },
    contextWindow: 500_000,
    maxTokens: 500_000,
    compat: { supportsStore: false, supportsDeveloperRole: false, supportsReasoningEffort: true },
    thinkingLevelMap: {
      off: null,
      minimal: null,
      low: "low",
      medium: "medium",
      high: "high",
      xhigh: "xhigh",
      max: null,
    },
  },
];

export function oauthCredential(credential: Credential): OAuthCredential {
  if (credential.type !== "oauth") {
    throw new Error("xAI credentials must be OAuth");
  }
  const oauth: OAuthCredential = {
    type: "oauth",
    access: credential.access,
    refresh: credential.refresh,
    expires: credential.expires,
  };
  if (credential.accountId !== undefined) {
    oauth.accountId = credential.accountId;
  }
  return oauth;
}

export function fromOauth(credential: OAuthCredential): Extract<Credential, { type: "oauth" }> {
  const stored: Extract<Credential, { type: "oauth" }> = {
    type: "oauth",
    access: credential.access,
    refresh: credential.refresh,
    expires: credential.expires,
  };
  if (typeof credential.accountId === "string") {
    return { ...stored, accountId: credential.accountId };
  }
  return stored;
}

export { xaiOAuth };
