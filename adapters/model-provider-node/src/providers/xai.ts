import type { OAuthAuth, OAuthCredential } from "@earendil-works/pi-ai";
import { xaiProvider } from "@earendil-works/pi-ai/providers/xai";

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

/** Pi's xAI device-code login and refresh, loaded on first use. */
export const xaiOAuth: OAuthAuth = requireOAuth(xaiProvider().auth.oauth);

function requireOAuth(oauth: OAuthAuth | undefined): OAuthAuth {
  if (oauth === undefined) {
    throw new Error("Pi's xAI provider has no OAuth login");
  }
  return oauth;
}
