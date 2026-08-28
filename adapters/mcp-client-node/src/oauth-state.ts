import type {
  OAuthClientInformationContext,
  OAuthClientMetadata,
  OAuthClientProvider,
  OAuthDiscoveryState,
  StoredOAuthClientInformation,
  StoredOAuthTokens,
} from "@modelcontextprotocol/client";
import type { WireOAuthState } from "./contract.js";
import { isLoopbackHost, parseEndpoint } from "./endpoint.js";
import { AdapterProblem } from "./errors.js";
import {
  MAX_AUTH_TOKEN_BYTES,
  MAX_OAUTH_STATE_BYTES,
  MAX_OAUTH_VALUE_BYTES,
} from "./limits.js";

const TOKEN_EXPIRY_SKEW_MS = 60_000;

interface PersistedOAuthState {
  schema_version: 1;
  mcp_endpoint: string;
  csrf_state: string;
  redirect_uri: string;
  authorization_url?: string;
  authorization_server_url?: string;
  client_information?: StoredOAuthClientInformation;
  code_verifier?: string;
  discovery_state?: OAuthDiscoveryState;
  resource_url?: string;
  tokens?: StoredOAuthTokens;
  tokens_saved_at_ms?: number;
}

export interface CurrentOAuthToken {
  readonly accessToken: string;
}

export class RenoaOAuthProvider implements OAuthClientProvider {
  readonly #state: PersistedOAuthState;

  constructor(state: WireOAuthState, endpoint: string) {
    this.#state = structuredClone(state) as unknown as PersistedOAuthState;
    validateCoreState(this.#state, canonicalEndpoint(endpoint));
  }

  static begin(
    prior: WireOAuthState | undefined,
    csrfState: string,
    redirectUri: string,
    forceReauthorization: boolean,
    endpoint: string,
  ): RenoaOAuthProvider {
    const mcpEndpoint = canonicalEndpoint(endpoint);
    const retained = prior === undefined || prior.mcp_endpoint !== mcpEndpoint
      ? {}
      : retainLongLivedState(
          prior as PersistedOAuthState,
          forceReauthorization,
        );
    return new RenoaOAuthProvider({
      schema_version: 1,
      mcp_endpoint: mcpEndpoint,
      csrf_state: csrfState,
      redirect_uri: redirectUri,
      ...retained,
    } as unknown as WireOAuthState, endpoint);
  }

  get redirectUrl(): string {
    return this.#state.redirect_uri;
  }

  get clientMetadata(): OAuthClientMetadata {
    return {
      redirect_uris: [this.#state.redirect_uri],
      token_endpoint_auth_method: "none",
      grant_types: ["authorization_code", "refresh_token"],
      response_types: ["code"],
      application_type: "native",
      client_name: "Renoa",
      software_id: "renoa",
      software_version: "0.1.0",
    };
  }

  state(): string {
    return this.#state.csrf_state;
  }

  clientInformation(
    _context?: OAuthClientInformationContext,
  ): StoredOAuthClientInformation | undefined {
    return clone(this.#state.client_information);
  }

  saveClientInformation(value: StoredOAuthClientInformation): void {
    this.#state.client_information = clone(value);
  }

  tokens(
    _context?: OAuthClientInformationContext,
  ): StoredOAuthTokens | undefined {
    return clone(this.#state.tokens);
  }

  saveTokens(tokens: StoredOAuthTokens): void {
    this.#state.tokens = clone(tokens);
    this.#state.tokens_saved_at_ms = Date.now();
  }

  redirectToAuthorization(url: URL): void {
    validateAuthorizationUrl(url, this.#state);
    this.#state.authorization_url = url.href;
  }

  saveCodeVerifier(verifier: string): void {
    requireBoundedSecret(verifier, "OAuth code verifier");
    this.#state.code_verifier = verifier;
  }

  codeVerifier(): string {
    const verifier = this.#state.code_verifier;
    if (verifier === undefined) {
      throw invalid("stored OAuth state has no PKCE verifier");
    }
    return verifier;
  }

  invalidateCredentials(
    scope: "all" | "client" | "tokens" | "verifier" | "discovery",
  ): void {
    if (scope === "all" || scope === "client") {
      delete this.#state.client_information;
    }
    if (scope === "all" || scope === "tokens") {
      delete this.#state.tokens;
      delete this.#state.tokens_saved_at_ms;
    }
    if (scope === "all" || scope === "verifier") {
      delete this.#state.code_verifier;
      delete this.#state.authorization_url;
    }
    if (scope === "all" || scope === "discovery") {
      delete this.#state.discovery_state;
      delete this.#state.authorization_server_url;
      delete this.#state.resource_url;
    }
  }

  saveAuthorizationServerUrl(url: string): void {
    this.#state.authorization_server_url = boundedUrl(url, "authorization server");
  }

  authorizationServerUrl(): string | undefined {
    return this.#state.authorization_server_url;
  }

  saveResourceUrl(url: string): void {
    this.#state.resource_url = boundedUrl(url, "OAuth resource");
  }

  resourceUrl(): string | undefined {
    return this.#state.resource_url;
  }

  saveDiscoveryState(state: OAuthDiscoveryState): void {
    this.#state.discovery_state = clone(state);
  }

  discoveryState(): OAuthDiscoveryState | undefined {
    return clone(this.#state.discovery_state);
  }

  authorizationUrl(): string {
    const url = this.#state.authorization_url;
    if (url === undefined) {
      throw invalid("OAuth SDK requested a redirect without an authorization URL");
    }
    return url;
  }

  currentToken(
    nowMs = Date.now(),
    expirySkewMs = TOKEN_EXPIRY_SKEW_MS,
  ): CurrentOAuthToken | undefined {
    const tokens = this.#state.tokens;
    if (tokens === undefined) {
      return undefined;
    }
    requireBoundedSecret(tokens.access_token, "OAuth access token");
    if (tokens.token_type.toLowerCase() !== "bearer") {
      throw invalid("OAuth server returned a non-Bearer access token");
    }
    const expiresAtMs = tokenExpiry(tokens, this.#state.tokens_saved_at_ms);
    if (expiresAtMs !== undefined && expiresAtMs <= nowMs + expirySkewMs) {
      return undefined;
    }
    return { accessToken: tokens.access_token };
  }

  hasTokens(): boolean {
    return this.#state.tokens !== undefined;
  }

  snapshot(): WireOAuthState {
    const state = clone(this.#state) as unknown as WireOAuthState;
    const bytes = Buffer.byteLength(JSON.stringify(state), "utf8");
    if (bytes > MAX_OAUTH_STATE_BYTES) {
      throw new AdapterProblem(
        "resource_limit",
        `OAuth state exceeds ${MAX_OAUTH_STATE_BYTES} bytes.`,
        { code: "oauth_state_limit" },
      );
    }
    return state;
  }
}

function retainLongLivedState(
  state: PersistedOAuthState,
  forceReauthorization: boolean,
): Partial<PersistedOAuthState> {
  return {
    ...(state.authorization_server_url === undefined
      ? {}
      : { authorization_server_url: state.authorization_server_url }),
    ...(state.client_information === undefined
      ? {}
      : { client_information: clone(state.client_information) }),
    ...(state.discovery_state === undefined
      ? {}
      : { discovery_state: clone(state.discovery_state) }),
    ...(state.resource_url === undefined
      ? {}
      : { resource_url: state.resource_url }),
    ...(forceReauthorization || state.tokens === undefined
      ? {}
      : { tokens: clone(state.tokens) }),
    ...(forceReauthorization || state.tokens_saved_at_ms === undefined
      ? {}
      : { tokens_saved_at_ms: state.tokens_saved_at_ms }),
  };
}

function validateCoreState(
  state: PersistedOAuthState,
  expectedEndpoint: string,
): void {
  if (state.mcp_endpoint !== expectedEndpoint) {
    throw invalid("OAuth credential state belongs to a different MCP endpoint");
  }
  requireBoundedSecret(state.csrf_state, "OAuth state parameter");
  const redirect = new URL(state.redirect_uri);
  const loopback = redirect.hostname === "127.0.0.1" || redirect.hostname === "[::1]";
  if (
    redirect.protocol !== "http:" ||
    !loopback ||
    redirect.pathname !== "/oauth/callback" ||
    redirect.username.length > 0 ||
    redirect.password.length > 0 ||
    redirect.search.length > 0 ||
    redirect.hash.length > 0
  ) {
    throw invalid("OAuth redirect URI must be an exact loopback HTTP callback");
  }
}

function canonicalEndpoint(value: string): string {
  return parseEndpoint(value).href;
}

function validateAuthorizationUrl(url: URL, state: PersistedOAuthState): void {
  if (url.protocol !== "https:" && !isLoopbackHttp(url)) {
    throw invalid("OAuth authorization URL must use HTTPS or loopback HTTP");
  }
  if (url.username.length > 0 || url.password.length > 0 || url.hash.length > 0) {
    throw invalid("OAuth authorization URL contains forbidden URL components");
  }
  if (url.searchParams.get("state") !== state.csrf_state) {
    throw invalid("OAuth authorization URL did not preserve the Host state parameter");
  }
  if (url.searchParams.get("redirect_uri") !== state.redirect_uri) {
    throw invalid("OAuth authorization URL did not preserve the Host redirect URI");
  }
  if (Buffer.byteLength(url.href, "utf8") > MAX_OAUTH_VALUE_BYTES) {
    throw invalid("OAuth authorization URL exceeds its boundary");
  }
}

function boundedUrl(value: string, kind: string): string {
  if (Buffer.byteLength(value, "utf8") > MAX_OAUTH_VALUE_BYTES) {
    throw invalid(`${kind} URL exceeds its boundary`);
  }
  return value;
}

function tokenExpiry(
  tokens: StoredOAuthTokens,
  savedAtMs: number | undefined,
): number | undefined {
  if (tokens.expires_in === undefined) {
    return undefined;
  }
  if (
    savedAtMs === undefined ||
    !Number.isSafeInteger(savedAtMs) ||
    !Number.isSafeInteger(tokens.expires_in) ||
    tokens.expires_in < 0
  ) {
    return 0;
  }
  const lifetimeMs = tokens.expires_in * 1_000;
  const expiresAtMs = savedAtMs + lifetimeMs;
  return Number.isSafeInteger(lifetimeMs) && Number.isSafeInteger(expiresAtMs)
    ? expiresAtMs
    : 0;
}

function requireBoundedSecret(value: string, kind: string): void {
  if (
    value.length === 0 ||
    Buffer.byteLength(value, "utf8") > MAX_AUTH_TOKEN_BYTES ||
    /[\u0000-\u001F\u007F]/u.test(value)
  ) {
    throw invalid(`${kind} is empty, malformed, or over limit`);
  }
}

function isLoopbackHttp(url: URL): boolean {
  return url.protocol === "http:" && isLoopbackHost(url.hostname);
}

function clone<T>(value: T): T {
  return structuredClone(value);
}

function invalid(message: string): AdapterProblem {
  return new AdapterProblem("invalid_request", message, {
    code: "invalid_oauth_state",
  });
}
