import type {
  OAuthClientInformationContext,
  OAuthClientMetadata,
  OAuthClientProvider,
  OAuthDiscoveryState,
  StoredOAuthClientInformation,
  StoredOAuthTokens,
} from "@modelcontextprotocol/client";
import type { WireOAuthRegistration, WireOAuthState } from "./contract.js";
import { AdapterProblem } from "./errors.js";
import { MAX_OAUTH_STATE_BYTES } from "./limits.js";
import { isValidOAuthScope } from "./oauth-scope.js";
import {
  bindIssuer,
  boundedUrl,
  canonicalEndpoint,
  canonicalIssuer,
  invalid,
  issuerMatches,
  normalizeCredentialIssuers,
  type PersistedOAuthState,
  requireBoundedSecret,
  requireIssuer,
  sameIssuer,
  tokenExpiry,
  validateAuthorizationUrl,
  validateCoreState,
} from "./oauth-state-validation.js";

const TOKEN_EXPIRY_SKEW_MS = 60_000;
const GOOGLE_OAUTH_ISSUER = "https://accounts.google.com";

export interface CurrentOAuthToken {
  readonly accessToken: string;
}

export class RenoaOAuthProvider implements OAuthClientProvider {
  readonly #state: PersistedOAuthState;
  readonly #registration: WireOAuthRegistration;
  readonly clientMetadataUrl?: string;
  readonly saveClientInformation?: (
    value: StoredOAuthClientInformation,
    context?: OAuthClientInformationContext,
  ) => void;

  constructor(
    state: WireOAuthState,
    endpoint: string,
    registration: WireOAuthRegistration,
  ) {
    this.#state = structuredClone(state) as unknown as PersistedOAuthState;
    this.#registration = structuredClone(registration);
    validateCoreState(this.#state, canonicalEndpoint(endpoint));
    normalizeCredentialIssuers(this.#state);
    if (registration.mode === "client_metadata") {
      this.clientMetadataUrl = registration.client_metadata_url;
    }
    if (
      registration.mode === "dynamic" ||
      registration.mode === "client_metadata"
    ) {
      this.saveClientInformation = (value, context) => {
        this.#state.client_information = bindIssuer(value, context);
      };
    }
  }

  static begin(
    prior: WireOAuthState | undefined,
    csrfState: string,
    redirectUri: string,
    forceReauthorization: boolean,
    endpoint: string,
    registration: WireOAuthRegistration,
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
    } as unknown as WireOAuthState, endpoint, registration);
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
    context?: OAuthClientInformationContext,
  ): StoredOAuthClientInformation | undefined {
    if (this.#registration.mode === "pre_registered") {
      requireIssuer(
        this.#registration.issuer,
        context,
        "pre-registered OAuth client",
      );
      return {
        client_id: this.#registration.client_id,
        ...(this.#registration.client_secret === undefined
          ? {}
          : { client_secret: this.#registration.client_secret }),
        issuer: canonicalIssuer(this.#registration.issuer),
      };
    }
    const client = this.#state.client_information;
    return issuerMatches(client?.issuer, context) ? clone(client) : undefined;
  }

  tokens(
    context?: OAuthClientInformationContext,
  ): StoredOAuthTokens | undefined {
    const tokens = this.#state.tokens;
    return issuerMatches(tokens?.issuer, context) ? clone(tokens) : undefined;
  }

  saveTokens(
    tokens: StoredOAuthTokens,
    context?: OAuthClientInformationContext,
  ): void {
    validateTokenScope(tokens.scope);
    if (tokens.scope !== undefined) {
      this.#state.oauth_scope = tokens.scope;
    }
    this.#state.tokens = bindIssuer(tokens, context);
    this.#state.tokens_saved_at_ms = Date.now();
  }

  redirectToAuthorization(url: URL): void {
    applyAuthorizationServerPolicy(url, this.#state);
    validateAuthorizationUrl(url, this.#state);
    const scope = url.searchParams.get("scope");
    if (scope === null) {
      delete this.#state.oauth_scope;
    } else {
      validateTokenScope(scope);
      this.#state.oauth_scope = scope;
    }
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
    if (scope === "all") {
      delete this.#state.oauth_scope;
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
    const issuer = canonicalIssuer(url);
    for (const stored of [
      this.#state.client_information?.issuer,
      this.#state.tokens?.issuer,
    ]) {
      if (stored !== undefined && !sameIssuer(stored, issuer)) {
        throw invalid(
          "OAuth authorization server changed after credentials were selected",
        );
      }
    }
    this.#state.authorization_server_url = issuer;
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
    if (tokens.issuer === undefined) {
      throw invalid("OAuth token is missing its authorization server issuer");
    }
    if (
      this.#state.authorization_server_url !== undefined &&
      !sameIssuer(tokens.issuer, this.#state.authorization_server_url)
    ) {
      throw invalid(
        "OAuth token is not bound to the current authorization server issuer",
      );
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

  grantedScope(): string | undefined {
    if (this.#state.tokens === undefined) {
      return undefined;
    }
    const scope = this.#state.tokens.scope ?? this.#state.oauth_scope;
    validateTokenScope(scope);
    return scope;
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

function applyAuthorizationServerPolicy(
  url: URL,
  state: PersistedOAuthState,
): void {
  if (!sameIssuer(state.authorization_server_url, GOOGLE_OAUTH_ISSUER)) return;

  url.searchParams.set("access_type", "offline");
  url.searchParams.set("include_granted_scopes", "true");
  const prompts = (url.searchParams.get("prompt") ?? "")
    .split(/\s+/u)
    .filter((prompt) => prompt.length > 0 && prompt !== "none");
  if (!prompts.includes("consent")) prompts.push("consent");
  url.searchParams.set("prompt", prompts.join(" "));
}

function validateTokenScope(scope: unknown): asserts scope is string | undefined {
  if (scope !== undefined && (typeof scope !== "string" || !isValidOAuthScope(scope))) {
    throw invalid("OAuth token contains an invalid scope grant");
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
    ...(state.oauth_scope === undefined ? {} : { oauth_scope: state.oauth_scope }),
    ...(forceReauthorization || state.tokens === undefined
      ? {}
      : { tokens: clone(state.tokens) }),
    ...(forceReauthorization || state.tokens_saved_at_ms === undefined
      ? {}
      : { tokens_saved_at_ms: state.tokens_saved_at_ms }),
  };
}

function clone<T>(value: T): T {
  return structuredClone(value);
}
