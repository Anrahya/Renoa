import assert from "node:assert/strict";
import { once } from "node:events";
import {
  createServer,
  type IncomingMessage,
  type ServerResponse,
} from "node:http";
import type { AdapterRequest, WireOAuthState } from "../src/contract.js";
import { WIRE_VERSION } from "../src/limits.js";
import { runAdapter } from "./support.js";

export const CALLBACK = "http://127.0.0.1:45831/oauth/callback";
export const CSRF = "host-generated-state-with-enough-entropy";
type OAuthBeginRequest = Extract<
  AdapterRequest,
  { readonly action: "oauth_begin" }
>;

export async function begin(server: OAuthFixture): Promise<WireOAuthState> {
  const result = await runAdapter(beginRequest(server.endpoint));
  const record = result.records[0];
  assert.equal(record?.event, "oauth_redirect", JSON.stringify(record));
  if (record?.event !== "oauth_redirect") {
    throw new Error("OAuth did not redirect");
  }
  return record.oauth_state;
}

export async function exchange(
  server: OAuthFixture,
): Promise<
  Extract<
    (Awaited<ReturnType<typeof runAdapter>>)["records"][number],
    { event: "oauth_authorized" }
  >
> {
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
  const record = result.records[0];
  assert.equal(record?.event, "oauth_authorized", JSON.stringify(record));
  if (record?.event !== "oauth_authorized") {
    throw new Error("OAuth did not authorize");
  }
  return record;
}

export function beginRequest(endpoint: string): OAuthBeginRequest {
  return {
    wire_version: WIRE_VERSION,
    action: "oauth_begin",
    endpoint,
    csrf_state: CSRF,
    redirect_uri: CALLBACK,
    force_reauthorization: false,
    registration: { mode: "dynamic" },
  };
}

interface OAuthFixtureOptions {
  readonly rejectRegistration?: boolean;
  readonly advertiseIssuerResponse?: boolean;
  readonly omitRegistrationEndpoint?: boolean;
  readonly clientMetadataSupported?: boolean;
  readonly tokenAuthMethods?: readonly string[];
}

export class OAuthFixture {
  registrationRequests = 0;
  tokenRequests = 0;
  refreshRequests = 0;
  requests = 0;
  tokenAuthorization: string | undefined;
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
    if (this.#origin === undefined) {
      throw new Error("fixture is not started");
    }
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
      this.#server.close((error) =>
        error === undefined ? resolve() : reject(error),
      );
    });
  }

  async #respond(
    request: IncomingMessage,
    response: ServerResponse,
  ): Promise<void> {
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
        ...(this.#options.omitRegistrationEndpoint === true
          ? {}
          : { registration_endpoint: `${this.origin}/register` }),
        response_types_supported: ["code"],
        grant_types_supported: ["authorization_code", "refresh_token"],
        scopes_supported: ["search"],
        code_challenge_methods_supported: ["S256"],
        token_endpoint_auth_methods_supported:
          this.#options.tokenAuthMethods ?? ["none"],
        ...(this.#options.clientMetadataSupported === true
          ? { client_id_metadata_document_supported: true }
          : {}),
        ...(this.#options.advertiseIssuerResponse === true
          ? { authorization_response_iss_parameter_supported: true }
          : {}),
      });
    }
    if (url.pathname === "/register") {
      this.registrationRequests += 1;
      const registration = JSON.parse(
        await body(request),
      ) as Record<string, unknown>;
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
      this.tokenAuthorization = request.headers.authorization;
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
