import assert from "node:assert/strict";
import test from "node:test";
import type { WireOAuthRegistration } from "../src/contract.js";
import { RenoaOAuthProvider } from "../src/oauth-state.js";

const ENDPOINT = "https://drivemcp.googleapis.com/mcp/v1";
const CALLBACK = "https://renoa.live/v1/oauth/callback";
const STATE = "host-generated-state-with-enough-entropy";

test("Google authorization requests durable offline access", () => {
  const provider = begin({
    mode: "pre_registered",
    issuer: "https://accounts.google.com",
    client_id: "google-client",
    client_secret: "google-secret",
  });
  provider.saveAuthorizationServerUrl("https://accounts.google.com/");
  const authorization = authorizationUrl(
    "https://accounts.google.com/o/oauth2/v2/auth",
  );
  authorization.searchParams.set("prompt", "select_account");

  provider.redirectToAuthorization(authorization);

  const saved = new URL(provider.authorizationUrl());
  assert.equal(saved.searchParams.get("access_type"), "offline");
  assert.equal(saved.searchParams.get("include_granted_scopes"), "true");
  assert.deepEqual(
    saved.searchParams.get("prompt")?.split(" "),
    ["select_account", "consent"],
  );
});

test("provider-specific authorization parameters do not leak to other issuers", () => {
  const provider = begin({
    mode: "pre_registered",
    issuer: "https://auth.example",
    client_id: "other-client",
  });
  provider.saveAuthorizationServerUrl("https://auth.example");
  const authorization = authorizationUrl("https://auth.example/authorize");

  provider.redirectToAuthorization(authorization);

  const saved = new URL(provider.authorizationUrl());
  assert.equal(saved.searchParams.has("access_type"), false);
  assert.equal(saved.searchParams.has("include_granted_scopes"), false);
  assert.equal(saved.searchParams.has("prompt"), false);
});

function begin(registration: WireOAuthRegistration): RenoaOAuthProvider {
  return RenoaOAuthProvider.begin(
    undefined,
    STATE,
    CALLBACK,
    false,
    ENDPOINT,
    registration,
  );
}

function authorizationUrl(origin: string): URL {
  const url = new URL(origin);
  url.searchParams.set("state", STATE);
  url.searchParams.set("redirect_uri", CALLBACK);
  return url;
}
