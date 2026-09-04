import assert from "node:assert/strict";
import test from "node:test";
import type { WireOAuthState } from "../src/contract.js";
import { WIRE_VERSION } from "../src/limits.js";
import { exchange, OAuthFixture } from "./oauth-fixture.js";
import { runAdapter } from "./support.js";

test("an expired grant without a refresh token gives one actionable failure", async () => {
  const server = new OAuthFixture();
  await server.start();
  try {
    const authorized = await exchange(server);
    const expired = structuredClone(authorized.oauth_state) as WireOAuthState & {
      tokens: { refresh_token?: string };
      tokens_saved_at_ms: number;
    };
    delete expired.tokens.refresh_token;
    expired.tokens_saved_at_ms = 0;
    const requestsBeforeRefresh = server.tokenRequests;

    const refreshed = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_refresh",
      endpoint: server.endpoint,
      registration: { mode: "dynamic" },
      oauth_state: expired,
    });

    const terminal = refreshed.records[0];
    assert.equal(terminal?.event, "oauth_failed", JSON.stringify(terminal));
    if (terminal?.event !== "oauth_failed") return;
    assert.equal(
      terminal.failure.diagnostic.code,
      "oauth_refresh_token_missing",
    );
    assert.equal(terminal.failure.certainty, "definite");
    assert.equal(terminal.failure.partial_changes_possible, false);
    assert.match(terminal.failure.message, /authorize with restart=true/u);
    assert.equal(server.tokenRequests, requestsBeforeRefresh);
    assert.equal(server.refreshRequests, 0);
  } finally {
    await server.close();
  }
});
