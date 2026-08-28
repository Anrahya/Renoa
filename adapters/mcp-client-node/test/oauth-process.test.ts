import assert from "node:assert/strict";
import { createServer, type IncomingMessage, type ServerResponse } from "node:http";
import { once } from "node:events";
import test from "node:test";
import type { AdapterRequest, WireOAuthState } from "../src/contract.js";
import { WIRE_VERSION } from "../src/limits.js";
import { OAuthExchangeTracker } from "../src/oauth-transport.js";
import { runAdapter } from "./support.js";

const CALLBACK = "http://127.0.0.1:45831/oauth/callback";
const CSRF = "host-generated-state-with-enough-entropy";
type OAuthBeginRequest = Extract<AdapterRequest, { readonly action: "oauth_begin" }>;

test("OAuth certainty tracks the credential POST, not earlier discovery responses", () => {
  const tracker = new OAuthExchangeTracker();
  tracker.markResponse("GET");
  tracker.markRequest("POST");
  assert.deepEqual(tracker.evidence(), {
    dispatchStarted: true,
    responseStarted: false,
  });
  tracker.markResponse("POST");
  assert.equal(tracker.evidence().responseStarted, true);
});

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

async function begin(server: OAuthFixture): Promise<WireOAuthState> {
  const result = await runAdapter(beginRequest(server.endpoint));
  const record = result.records[0];
  assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
  if (record?.event !== "oauth_redirect") throw new Error("OAuth did not redirect");
  return record.oauth_state;
}

async function exchange(
  server: OAuthFixture,
): Promise<Extract<(Awaited<ReturnType<typeof runAdapter>>)["records"][number], { event: "oauth_authorized" }>> {
  const state = await begin(server);
  const result = await runAdapter({
    wire_version: WIRE_VERSION,
    action: "oauth_exchange",
    endpoint: server.endpoint,
    authorization_code: "one-time-code",
    issuer: server.origin,
    oauth_state: state,
  });
  const record = result.records[0];
  assert.equal(record?.event, "oauth_authorized", JSON.stringify(record));
  if (record?.event !== "oauth_authorized") throw new Error("OAuth did not authorize");
  return record;
}

function beginRequest(endpoint: string): OAuthBeginRequest {
  return {
    wire_version: WIRE_VERSION,
    action: "oauth_begin",
    endpoint,
    csrf_state: CSRF,
    redirect_uri: CALLBACK,
    force_reauthorization: false,
  };
}

interface OAuthFixtureOptions {
  readonly rejectRegistration?: boolean;
  readonly advertiseIssuerResponse?: boolean;
}

class OAuthFixture {
  registrationRequests = 0;
  tokenRequests = 0;
  refreshRequests = 0;
  requests = 0;
  readonly #server = createServer((request, response) => {
    void this.#respond(request, response).catch((error: unknown) => {
      response.writeHead(500, { "content-type": "text/plain" });
      response.end(error instanceof Error ? error.message : String(error));
    });
  });
  readonly #options: OAuthFixtureOptions;
  #origin: string | undefined;

  constructor(options: OAuthFixtureOptions = {}) {
    this.#options = options;
  }

  get origin(): string {
    if (this.#origin === undefined) throw new Error("fixture is not started");
    return this.#origin;
  }

  get endpoint(): string {
    return `${this.origin}/mcp`;
  }

  async start(): Promise<void> {
    this.#server.listen(0, "127.0.0.1");
    await once(this.#server, "listening");
    const address = this.#server.address();
    if (address === null || typeof address === "string") {
      throw new Error("fixture did not bind");
    }
    this.#origin = `http://127.0.0.1:${address.port}`;
  }

  async close(): Promise<void> {
    this.#server.closeAllConnections();
    await new Promise<void>((resolve, reject) => {
      this.#server.close((error) => error === undefined ? resolve() : reject(error));
    });
  }

  async #respond(request: IncomingMessage, response: ServerResponse): Promise<void> {
    this.requests += 1;
    const url = new URL(request.url ?? "/", this.origin);
    if (url.pathname.includes(".well-known/oauth-protected-resource")) {
      return json(response, 200, {
        resource: this.endpoint,
        authorization_servers: [this.origin],
        scopes_supported: ["search"],
      });
    }
    if (url.pathname.includes(".well-known/oauth-authorization-server")) {
      return json(response, 200, {
        issuer: this.origin,
        authorization_endpoint: `${this.origin}/authorize`,
        token_endpoint: `${this.origin}/token`,
        registration_endpoint: `${this.origin}/register`,
        response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        scopes_supported: ["search"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported: ["none"],
        ...(this.#options.advertiseIssuerResponse === true
          ? { authorization_response_iss_parameter_supported: true }
          : {}),
      });
    }
    if (url.pathname === "/register") {
      this.registrationRequests += 1;
      const registration = JSON.parse(await body(request)) as Record<string, unknown>;
      if (this.#options.rejectRegistration === true) {
        return json(response, 400, {
          error: "invalid_client",
          error_description: "server-client-secret",
        });
      }
      return json(response, 201, {
        ...registration,
        client_id: "renoa-fixture-client",
      });
    }
    if (url.pathname === "/token") {
      this.tokenRequests += 1;
      const params = new URLSearchParams(await body(request));
      if (params.get("grant_type") === "refresh_token") {
        this.refreshRequests += 1;
        assert.equal(params.get("refresh_token"), "refresh-one");
        return json(response, 200, {
          access_token: "access-two",
          refresh_token: "refresh-two",
          token_type: "Bearer",
          expires_in: 3600,
          scope: "search",
        });
      }
      assert.equal(params.get("code"), "one-time-code");
      assert.equal(params.get("redirect_uri"), CALLBACK);
      assert.notEqual(params.get("code_verifier"), null);
      return json(response, 200, {
        access_token: "access-one",
        refresh_token: "refresh-one",
        token_type: "Bearer",
        expires_in: 3600,
        scope: "search",
      });
    }
    return json(response, 404, { error: "not_found" });
  }
}

function json(response: ServerResponse, status: number, value: unknown): void {
  response.writeHead(status, { "content-type": "application/json" });
  response.end(JSON.stringify(value));
}

async function body(request: IncomingMessage): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}
