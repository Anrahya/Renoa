import assert from "node:assert/strict";
import test from "node:test";
import { WIRE_VERSION } from "../src/limits.js";
import { beginRequest, exchange, OAuthFixture } from "./oauth-fixture.js";
import { runAdapter } from "./support.js";

test("initial OAuth falls back to every scope advertised by the MCP resource", async () => {
  const scopes = ["items.read", "items.write", "offline.access"];
  const server = new OAuthFixture({
    resourceScopes: scopes,
    authorizationScopes: scopes,
  });
  await server.start();
  try {
    const result = await runAdapter(beginRequest(server.endpoint));
    const record = result.records[0];
    assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
    if (record?.event !== "oauth_redirect") return;
    assert.equal(
      new URL(record.authorization_url).searchParams.get("scope"),
      scopes.join(" "),
    );
  } finally {
    await server.close();
  }
});

test("scope step-up preserves the grant and forces fresh consent without re-registering", async () => {
  const server = new OAuthFixture({
    resourceScopes: ["search"],
    authorizationScopes: ["search", "items.write"],
  });
  await server.start();
  try {
    const authorized = await exchange(server);
    assert.equal(server.registrationRequests, 1);

    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      requested_scope: "items.write",
      oauth_state: authorized.oauth_state,
    });
    const redirect = result.records[0];
    assert.equal(redirect?.event, "oauth_redirect", JSON.stringify(redirect));
    if (redirect?.event !== "oauth_redirect") return;
    assert.equal(
      new URL(redirect.authorization_url).searchParams.get("scope"),
      "search items.write",
    );
    assert.equal(server.registrationRequests, 1);
    assert.equal(JSON.stringify(redirect).includes("access-one"), false);
    assert.equal(JSON.stringify(redirect).includes("refresh-one"), false);
  } finally {
    await server.close();
  }
});

test("scope step-up preserves the requested grant when the token omits scope", async () => {
  const server = new OAuthFixture({
    resourceScopes: ["items.read", "users.read"],
    authorizationScopes: ["items.read", "users.read", "items.write"],
    omitTokenScope: true,
  });
  await server.start();
  try {
    const authorized = await exchange(server);
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      requested_scope: "items.write",
      oauth_state: authorized.oauth_state,
    });
    const redirect = result.records[0];
    assert.equal(redirect?.event, "oauth_redirect", JSON.stringify(redirect));
    if (redirect?.event !== "oauth_redirect") return;
    assert.equal(
      new URL(redirect.authorization_url).searchParams.get("scope"),
      "items.read users.read items.write",
    );
  } finally {
    await server.close();
  }
});

test("malformed requested scopes fail before OAuth discovery", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_begin",
      endpoint: server.endpoint,
      csrf_state: "scope-state",
      redirect_uri: "http://127.0.0.1:45831/oauth/callback",
      force_reauthorization: false,
      requested_scope: "items.read  items.write",
      registration: { mode: "dynamic" },
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(record.failure.kind, "invalid_request");
    assert.equal(server.requests, 0);
  } finally {
    await server.close();
  }
});
