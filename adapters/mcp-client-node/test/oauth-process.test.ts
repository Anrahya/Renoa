import assert from "node:assert/strict";
import test from "node:test";
import type { WireOAuthState } from "../src/contract.js";
import { WIRE_VERSION } from "../src/limits.js";
import {
  begin,
  beginRequest,
  CALLBACK,
  CSRF,
  exchange,
  OAuthFixture,
} from "./oauth-fixture.js";
import { runAdapter } from "./support.js";

test("OAuth begins with discovery, one registration, PKCE, and a durable redirect state", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const result = await runAdapter(beginRequest(server.endpoint));
    assert.equal(result.exitCode, 0, result.stderr);
    assert.equal(result.records.length, 1);
    const record = result.records[0];
    assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
    if (record?.event !== "oauth_redirect") return;
    const authorization = new URL(record.authorization_url);
    assert.equal(authorization.origin, server.origin);
    assert.equal(authorization.pathname, "/authorize");
    assert.equal(authorization.searchParams.get("state"), CSRF);
    assert.equal(authorization.searchParams.get("redirect_uri"), CALLBACK);
    assert.equal(authorization.searchParams.get("code_challenge_method"), "S256");
    assert.equal(record.oauth_state.csrf_state, CSRF);
    assert.equal(record.oauth_state.redirect_uri, CALLBACK);
    assert.equal(server.registrationRequests, 1);
    assert.equal(server.tokenRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth accepts an exact HTTPS Renoa callback relay", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const callback = "https://renoa.live/v1/oauth/callback";
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      redirect_uri: callback,
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
    if (record?.event !== "oauth_redirect") return;
    assert.equal(record.oauth_state.redirect_uri, callback);
    assert.equal(
      new URL(record.authorization_url).searchParams.get("redirect_uri"),
      callback,
    );
  } finally {
    await server.close();
  }
});

test("Client ID Metadata Documents skip dynamic registration when advertised", async () => {
  const server = new OAuthFixture({
    omitRegistrationEndpoint: true,
    clientMetadataSupported: true,
  });
  await server.start();
  try {
    const clientMetadataUrl = "https://renoa.example/oauth/client-metadata.json";
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      registration: {
        mode: "client_metadata",
        client_metadata_url: clientMetadataUrl,
      },
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
    if (record?.event !== "oauth_redirect") return;
    assert.equal(
      new URL(record.authorization_url).searchParams.get("client_id"),
      clientMetadataUrl,
    );
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("Client ID Metadata Documents fall back to dynamic registration when supported", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      registration: {
        mode: "client_metadata",
        client_metadata_url: "https://renoa.example/oauth/client-metadata.json",
      },
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
    assert.equal(server.registrationRequests, 1);
  } finally {
    await server.close();
  }
});

test("pre-registered clients skip registration and authenticate the token exchange", async () => {
  const server = new OAuthFixture({
    omitRegistrationEndpoint: true,
    tokenAuthMethods: ["client_secret_basic"],
  });
  await server.start();
  const registration = {
    mode: "pre_registered" as const,
    issuer: server.origin,
    client_id: "google-desktop-client",
    client_secret: "google-client-secret",
  };
  try {
    const started = await runAdapter({
      ...beginRequest(server.endpoint),
      registration,
    });
    const redirect = started.records[0];
    assert.equal(redirect?.event, "oauth_redirect", JSON.stringify(redirect));
    if (redirect?.event !== "oauth_redirect") return;
    assert.equal(
      new URL(redirect.authorization_url).searchParams.get("client_id"),
      registration.client_id,
    );
    assert.equal(server.registrationRequests, 0);

    const exchanged = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_exchange",
      endpoint: server.endpoint,
      authorization_code: "one-time-code",
      issuer: server.origin,
      registration,
      oauth_state: redirect.oauth_state,
    });
    assert.equal(exchanged.records[0]?.event, "oauth_authorized");
    assert.equal(
      server.tokenAuthorization,
      `Basic ${Buffer.from(`${registration.client_id}:${registration.client_secret}`).toString("base64")}`,
    );
    assert.equal(server.registrationRequests, 0);
    assert.equal(
      JSON.stringify(exchanged.records[0]).includes(registration.client_secret),
      false,
    );
  } finally {
    await server.close();
  }
});

test("pre-registered clients are rejected for a different issuer", async () => {
  const server = new OAuthFixture({ omitRegistrationEndpoint: true });
  await server.start();
  try {
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      registration: {
        mode: "pre_registered",
        issuer: "https://different.example",
        client_id: "wrong-server-client",
      },
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_failed", JSON.stringify(record));
    assert.equal(server.registrationRequests, 0);
    assert.equal(server.tokenRequests, 0);
  } finally {
    await server.close();
  }
});

test("dynamic registration reports the required setup when the server has no endpoint", async () => {
  const server = new OAuthFixture({ omitRegistrationEndpoint: true });
  await server.start();
  try {
    const result = await runAdapter(beginRequest(server.endpoint));
    const record = result.records[0];
    assert.equal(record?.event, "oauth_failed", JSON.stringify(record));
    if (record?.event !== "oauth_failed") return;
    assert.equal(record.failure.diagnostic.code, "oauth_registration_required");
    assert.equal(record.failure.partial_changes_possible, false);
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth exchanges one code and a later local token read performs no network request", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const state = await begin(server);
    const exchanged = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_exchange",
      endpoint: server.endpoint,
      authorization_code: "one-time-code",
      issuer: server.origin,
      registration: { mode: "dynamic" },
      oauth_state: state,
    });
    const authorized = exchanged.records[0];
    assert.equal(authorized?.event, "oauth_authorized", JSON.stringify(authorized));
    if (authorized?.event !== "oauth_authorized") return;
    assert.equal(authorized.authorization.token, "access-one");
    assert.equal(server.tokenRequests, 1);

    const before = server.requests;
    const loaded = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: authorized.oauth_state,
    });
    assert.equal(loaded.records[0]?.event, "oauth_authorized");
    assert.equal(server.requests, before);
  } finally {
    await server.close();
  }
});

test("OAuth refresh rotates once and returns the replacement state", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const expired = structuredClone(authorized.oauth_state) as WireOAuthState & {
      tokens_saved_at_ms: number;
    };
    expired.tokens_saved_at_ms = 0;
    const needsRefresh = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: expired,
    });
    assert.equal(needsRefresh.records[0]?.event, "oauth_refresh_required");

    const refreshed = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_refresh",
      endpoint: server.endpoint,
      registration: { mode: "dynamic" },
      oauth_state: expired,
    });
    const terminal = refreshed.records[0];
    assert.equal(terminal?.event, "oauth_authorized", JSON.stringify(terminal));
    if (terminal?.event !== "oauth_authorized") return;
    assert.equal(terminal.authorization.token, "access-two");
    assert.equal(server.refreshRequests, 1);
  } finally {
    await server.close();
  }
});

test("explicit reauthorization drops cached tokens without registering a second client", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    assert.equal(server.registrationRequests, 1);
    const result = await runAdapter({
      ...beginRequest(server.endpoint),
      force_reauthorization: true,
      oauth_state: authorized.oauth_state,
    });
    const redirect = result.records[0];
    assert.equal(redirect?.event, "oauth_redirect", JSON.stringify(redirect));
    assert.equal(server.registrationRequests, 1);
    assert.equal(JSON.stringify(redirect).includes("access-one"), false);
    assert.equal(JSON.stringify(redirect).includes("refresh-one"), false);
  } finally {
    await server.close();
  }
});

test("stored OAuth credentials cannot be read for a different MCP endpoint", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: "http://127.0.0.1:9/different-mcp",
      oauth_state: authorized.oauth_state,
    });
    assert.equal(result.records[0]?.event, "failed");
    assert.equal(
      JSON.stringify(result.records[0]).includes("authorization"),
      false,
    );
  } finally {
    await server.close();
  }
});

test("stored OAuth tokens cannot cross authorization-server issuers", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const rebound = structuredClone(authorized.oauth_state) as WireOAuthState & {
      authorization_server_url: string;
    };
    rebound.authorization_server_url = "https://attacker.example";
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: rebound,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(record.failure.diagnostic.code, "invalid_oauth_state");
  } finally {
    await server.close();
  }
});

test("version-5 OAuth state is bound to its validated issuer without reauthorization", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const legacy = structuredClone(authorized.oauth_state) as unknown as {
      authorization_server_url: string;
      client_information: { issuer?: string };
      tokens: { issuer?: string };
    };
    delete legacy.client_information.issuer;
    delete legacy.tokens.issuer;

    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: legacy as unknown as WireOAuthState,
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_authorized", JSON.stringify(record));
    if (record?.event !== "oauth_authorized") return;
    const migrated = record.oauth_state as unknown as {
      client_information: { issuer?: string };
      tokens: { issuer?: string };
    };
    assert.equal(migrated.client_information.issuer, server.origin);
    assert.equal(migrated.tokens.issuer, server.origin);
  } finally {
    await server.close();
  }
});

test("unbound legacy OAuth tokens are never used without a validated issuer", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const unbound = structuredClone(authorized.oauth_state) as unknown as {
      authorization_server_url?: string;
      client_information: { issuer?: string };
      tokens: { issuer?: string };
    };
    delete unbound.authorization_server_url;
    delete unbound.client_information.issuer;
    delete unbound.tokens.issuer;

    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: unbound as unknown as WireOAuthState,
    });
    assert.equal(result.records[0]?.event, "oauth_refresh_required");
  } finally {
    await server.close();
  }
});

test("a malformed stored callback is an invalid OAuth state", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const malformed = structuredClone(authorized.oauth_state) as WireOAuthState & {
      redirect_uri: string;
    };
    malformed.redirect_uri = "not a URL";

    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: server.endpoint,
      oauth_state: malformed,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(record.failure.kind, "invalid_request");
    assert.equal(record.failure.diagnostic.code, "invalid_oauth_state");
  } finally {
    await server.close();
  }
});

test("OAuth never repeats a failed registration inside one adapter request", async () => {
  const server = new OAuthFixture({ rejectRegistration: true });
  await server.start();
  try {
    const result = await runAdapter(beginRequest(server.endpoint));
    const terminal = result.records[0];
    assert.equal(terminal?.event, "oauth_failed", JSON.stringify(terminal));
    if (terminal?.event !== "oauth_failed") return;
    assert.equal(server.registrationRequests, 1);
    assert.equal(terminal.failure.diagnostic.code, "invalid_client");
    assert.equal(JSON.stringify(result).includes("server-client-secret"), false);
  } finally {
    await server.close();
  }
});

test("OAuth callback issuer mismatch fails before sending the code", async () => {
  const server = new OAuthFixture({ advertiseIssuerResponse: true });
  await server.start();
  try {
    const state = await begin(server);
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_exchange",
      endpoint: server.endpoint,
      authorization_code: "must-not-be-sent",
      issuer: "https://attacker.example",
      registration: { mode: "dynamic" },
      oauth_state: state,
    });
    const terminal = result.records[0];
    assert.equal(terminal?.event, "oauth_failed", JSON.stringify(terminal));
    assert.equal(server.tokenRequests, 0);
    assert.equal(JSON.stringify(result).includes("must-not-be-sent"), false);
  } finally {
    await server.close();
  }
});

test("OAuth returns the first token rejection without retrying the code", async () => {
  const server = new OAuthFixture({
    rejectToken: {
      status: 400,
      error: "invalid_grant",
      description: "authorization code already redeemed",
    },
  });
  await server.start();
  try {
    const state = await begin(server);
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_exchange",
      endpoint: server.endpoint,
      authorization_code: "one-time-code",
      issuer: server.origin,
      registration: { mode: "dynamic" },
      oauth_state: state,
    });
    const terminal = result.records[0];
    assert.equal(terminal?.event, "oauth_failed", JSON.stringify(terminal));
    if (terminal?.event !== "oauth_failed") return;
    assert.equal(server.tokenRequests, 1);
    assert.equal(terminal.failure.diagnostic.code, "invalid_grant");
    assert.match(
      terminal.failure.diagnostic.detail,
      /authorization code already redeemed/u,
    );
    assert.notEqual(
      terminal.failure.diagnostic.code,
      "hidden_oauth_retry_blocked",
      "the hidden retry guard must not replace the provider error",
    );
  } finally {
    await server.close();
  }
});
