import { xaiOAuth, type OAuthCredential } from "../upstream/oauth-xai.js";
import type { Credential } from "../credentials.js";

export const XAI_BASE_URL = "https://api.x.ai/v1";

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
