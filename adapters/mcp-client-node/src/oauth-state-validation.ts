import type {
  OAuthClientInformationContext,
  OAuthDiscoveryState,
  StoredOAuthClientInformation,
  StoredOAuthTokens,
} from "@modelcontextprotocol/client";
import { isLoopbackHost, parseEndpoint } from "./endpoint.js";
import { AdapterProblem } from "./errors.js";
import { MAX_AUTH_TOKEN_BYTES, MAX_OAUTH_VALUE_BYTES } from "./limits.js";

export interface PersistedOAuthState {
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

export function validateCoreState(
  state: PersistedOAuthState,
  expectedEndpoint: string,
): void {
  if (state.mcp_endpoint !== expectedEndpoint) {
    throw invalid("OAuth credential state belongs to a different MCP endpoint");
  }
  requireBoundedSecret(state.csrf_state, "OAuth state parameter");
  let redirect: URL;
  try {
    redirect = new URL(state.redirect_uri);
  } catch {
    throw invalid("OAuth redirect URI is not a valid URL");
  }
  const loopback =
    redirect.hostname === "127.0.0.1" || redirect.hostname === "[::1]";
  const httpsRelay =
    redirect.protocol === "https:" &&
    redirect.hostname.length > 0 &&
    redirect.pathname === "/v1/oauth/callback";
  if (
    !(
      (redirect.protocol === "http:" &&
        loopback &&
        redirect.pathname === "/oauth/callback") ||
      httpsRelay
    ) ||
    redirect.username.length > 0 ||
    redirect.password.length > 0 ||
    redirect.search.length > 0 ||
    redirect.hash.length > 0
  ) {
    throw invalid(
      "OAuth redirect URI must be an exact loopback or HTTPS Renoa callback",
    );
  }
}

export function normalizeCredentialIssuers(state: PersistedOAuthState): void {
  const authorizationServer =
    state.authorization_server_url === undefined
      ? undefined
      : canonicalIssuer(state.authorization_server_url);
  if (authorizationServer !== undefined) {
    state.authorization_server_url = authorizationServer;
  }

  if (state.client_information !== undefined) {
    const issuer = normalizedStoredIssuer(
      state.client_information.issuer,
      authorizationServer,
    );
    if (issuer === undefined) {
      delete state.client_information;
    } else {
      state.client_information = { ...state.client_information, issuer };
    }
  }
  if (state.tokens !== undefined) {
    const issuer = normalizedStoredIssuer(
      state.tokens.issuer,
      authorizationServer,
    );
    if (issuer === undefined) {
      delete state.tokens;
      delete state.tokens_saved_at_ms;
    } else {
      state.tokens = { ...state.tokens, issuer };
    }
  }
}

export function canonicalEndpoint(value: string): string {
  return parseEndpoint(value).href;
}

export function validateAuthorizationUrl(
  url: URL,
  state: PersistedOAuthState,
): void {
  if (url.protocol !== "https:" && !isLoopbackHttp(url)) {
    throw invalid("OAuth authorization URL must use HTTPS or loopback HTTP");
  }
  if (
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.hash.length > 0
  ) {
    throw invalid("OAuth authorization URL contains forbidden URL components");
  }
  if (url.searchParams.get("state") !== state.csrf_state) {
    throw invalid(
      "OAuth authorization URL did not preserve the Host state parameter",
    );
  }
  if (url.searchParams.get("redirect_uri") !== state.redirect_uri) {
    throw invalid("OAuth authorization URL did not preserve the Host redirect URI");
  }
  if (Buffer.byteLength(url.href, "utf8") > MAX_OAUTH_VALUE_BYTES) {
    throw invalid("OAuth authorization URL exceeds its boundary");
  }
}

export function boundedUrl(value: string, kind: string): string {
  if (Buffer.byteLength(value, "utf8") > MAX_OAUTH_VALUE_BYTES) {
    throw invalid(`${kind} URL exceeds its boundary`);
  }
  return value;
}

export function bindIssuer<T extends { readonly issuer?: string }>(
  value: T,
  context: OAuthClientInformationContext | undefined,
): T {
  const contextIssuer = context?.issuer;
  if (
    contextIssuer !== undefined &&
    value.issuer !== undefined &&
    !sameIssuer(value.issuer, contextIssuer)
  ) {
    throw invalid("OAuth SDK supplied conflicting authorization server issuers");
  }
  const issuer = contextIssuer ?? value.issuer;
  if (issuer === undefined) {
    throw invalid("OAuth credential is missing its authorization server issuer");
  }
  return { ...structuredClone(value), issuer: canonicalIssuer(issuer) };
}

export function issuerMatches(
  storedIssuer: string | undefined,
  context: OAuthClientInformationContext | undefined,
): boolean {
  return context === undefined
    ? storedIssuer !== undefined
    : sameIssuer(storedIssuer, context.issuer);
}

export function requireIssuer(
  configuredIssuer: string,
  context: OAuthClientInformationContext | undefined,
  kind: string,
): void {
  if (context !== undefined && !sameIssuer(configuredIssuer, context.issuer)) {
    throw invalid(`${kind} belongs to a different authorization server issuer`);
  }
}

export function sameIssuer(left: string | undefined, right: string): boolean {
  return left !== undefined && canonicalIssuer(left) === canonicalIssuer(right);
}

export function canonicalIssuer(value: string): string {
  let issuer: URL;
  try {
    issuer = new URL(value);
  } catch {
    throw invalid("OAuth authorization server issuer is not a valid URL");
  }
  if (
    (issuer.protocol !== "https:" && !isLoopbackHttp(issuer)) ||
    issuer.username.length > 0 ||
    issuer.password.length > 0 ||
    issuer.search.length > 0 ||
    issuer.hash.length > 0
  ) {
    throw invalid("OAuth authorization server issuer is not a safe issuer URL");
  }
  return issuer.pathname === "/" ? issuer.origin : issuer.href;
}

export function tokenExpiry(
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

export function requireBoundedSecret(value: string, kind: string): void {
  if (
    value.length === 0 ||
    Buffer.byteLength(value, "utf8") > MAX_AUTH_TOKEN_BYTES ||
    /[\u0000-\u001F\u007F]/u.test(value)
  ) {
    throw invalid(`${kind} is empty, malformed, or over limit`);
  }
}

function normalizedStoredIssuer(
  stored: string | undefined,
  authorizationServer: string | undefined,
): string | undefined {
  if (stored === undefined) {
    return authorizationServer;
  }
  const issuer = canonicalIssuer(stored);
  if (
    authorizationServer !== undefined &&
    !sameIssuer(issuer, authorizationServer)
  ) {
    throw invalid(
      "OAuth credential belongs to a different authorization server issuer",
    );
  }
  return issuer;
}

function isLoopbackHttp(url: URL): boolean {
  return url.protocol === "http:" && isLoopbackHost(url.hostname);
}

export function invalid(message: string): AdapterProblem {
  return new AdapterProblem("invalid_request", message, {
    code: "invalid_oauth_state",
  });
}
