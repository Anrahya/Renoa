import {
  auth,
  fetchToken,
  IssuerMismatchError,
  OAuthClientFlowError,
  OAuthError,
  refreshAuthorization,
  RegistrationRejectedError,
} from "@modelcontextprotocol/client";
import type {
  AdapterRecord,
  AdapterRequest,
  WireFailure,
  WireOAuthState,
} from "./contract.js";
import { toWireFailure } from "./errors.js";
import { WIRE_VERSION } from "./limits.js";
import { discoverOAuth } from "./oauth-discovery.js";
import { scopeUpgrade } from "./oauth-scope.js";
import { RenoaOAuthProvider } from "./oauth-state.js";
import { canonicalIssuer, sameIssuer } from "./oauth-state-validation.js";
import { guardedOAuthFetch, OAuthExchangeTracker } from "./oauth-transport.js";

type OAuthRequest = Exclude<
  AdapterRequest,
  { readonly action: "discover" | "call" }
>;

type OAuthFlowRequest = Exclude<
  OAuthRequest,
  { readonly action: "oauth_discover" }
>;

export function isOAuthRequest(request: AdapterRequest): request is OAuthRequest {
  return request.action.startsWith("oauth_");
}

export async function executeOAuthRequest(
  request: OAuthRequest,
  signal: AbortSignal,
): Promise<AdapterRecord> {
  const tracker = new OAuthExchangeTracker();
  if (request.action === "oauth_discover") {
    try {
      return {
        wire_version: WIRE_VERSION,
        event: "oauth_discovered",
        discovery: await discoverOAuth(
          request.endpoint,
          guardedOAuthFetch(tracker, signal),
        ),
      };
    } catch (error) {
      return {
        wire_version: WIRE_VERSION,
        event: "failed",
        failure: oauthFailure(error, tracker, signal.aborted),
      };
    }
  }
  const providerContext = providerFor(request);
  const provider = providerContext.provider;
  try {
    if (request.action === "oauth_token") {
      const token = provider.currentToken();
      return token === undefined
        ? {
            wire_version: WIRE_VERSION,
            event: "oauth_refresh_required",
            oauth_state: provider.snapshot(),
          }
        : authorized(provider, token.accessToken);
    }
    const fetchFn = guardedOAuthFetch(tracker, signal);
    const result = request.action === "oauth_exchange"
      ? await exchangeOnce(provider, request, fetchFn)
      : request.action === "oauth_refresh"
        ? await refreshOnce(provider, fetchFn)
        : await auth(provider, {
            serverUrl: request.endpoint,
            ...(providerContext.scope === undefined
              ? {}
              : { scope: providerContext.scope }),
            forceReauthorization: providerContext.forceReauthorization,
            fetchFn,
          });
    if (result === "REDIRECT") {
      return {
        wire_version: WIRE_VERSION,
        event: "oauth_redirect",
        authorization_url: provider.authorizationUrl(),
        oauth_state: provider.snapshot(),
      };
    }
    const token = provider.currentToken(Date.now(), 0);
    if (token === undefined) {
      throw new Error("OAuth completed without a usable Bearer token");
    }
    return authorized(provider, token.accessToken);
  } catch (error) {
    const state = provider.snapshot();
    const exactSecrets = oauthSecrets(state);
    if (
      "registration" in request &&
      request.registration.mode === "pre_registered" &&
      request.registration.client_secret !== undefined
    ) {
      exactSecrets.push(request.registration.client_secret);
    }
    if (request.action === "oauth_exchange") {
      exactSecrets.push(request.authorization_code);
    }
    return {
      wire_version: WIRE_VERSION,
      event: "oauth_failed",
      failure: redactExactSecrets(
        oauthFailure(error, tracker, signal.aborted),
        exactSecrets,
      ),
      oauth_state: state,
    };
  }
}

type OAuthFetch = ReturnType<typeof guardedOAuthFetch>;

async function exchangeOnce(
  provider: RenoaOAuthProvider,
  request: Extract<OAuthRequest, { readonly action: "oauth_exchange" }>,
  fetchFn: OAuthFetch,
): Promise<"AUTHORIZED"> {
  const context = tokenContext(provider);
  const tokens = await fetchToken(provider, context.authorizationServerUrl, {
    ...(context.metadata === undefined ? {} : { metadata: context.metadata }),
    ...(context.resource === undefined ? {} : { resource: context.resource }),
    authorizationCode: request.authorization_code,
    ...(request.issuer === undefined ? {} : { iss: request.issuer }),
    fetchFn,
  });
  provider.saveTokens({ ...tokens, issuer: context.issuer }, {
    issuer: context.issuer,
  });
  return "AUTHORIZED";
}

async function refreshOnce(
  provider: RenoaOAuthProvider,
  fetchFn: OAuthFetch,
): Promise<"AUTHORIZED"> {
  const context = tokenContext(provider);
  const tokens = provider.tokens({ issuer: context.issuer });
  const refreshToken = tokens?.refresh_token;
  if (refreshToken === undefined) {
    throw new OAuthClientFlowError("stored OAuth state has no refresh token");
  }
  const clientInformation = provider.clientInformation({
    issuer: context.issuer,
  });
  if (clientInformation === undefined) {
    throw new OAuthClientFlowError("stored OAuth state has no client information");
  }
  const refreshed = await refreshAuthorization(context.authorizationServerUrl, {
    ...(context.metadata === undefined ? {} : { metadata: context.metadata }),
    clientInformation,
    refreshToken,
    ...(context.resource === undefined ? {} : { resource: context.resource }),
    fetchFn,
  });
  provider.saveTokens({ ...refreshed, issuer: context.issuer }, {
    issuer: context.issuer,
  });
  return "AUTHORIZED";
}

function tokenContext(provider: RenoaOAuthProvider): {
  readonly authorizationServerUrl: string;
  readonly issuer: string;
  readonly metadata: NonNullable<
    ReturnType<RenoaOAuthProvider["discoveryState"]>
  >["authorizationServerMetadata"];
  readonly resource: URL | undefined;
} {
  const discovery = provider.discoveryState();
  const authorizationServerUrl = provider.authorizationServerUrl();
  if (
    discovery === undefined ||
    authorizationServerUrl === undefined ||
    !sameIssuer(discovery.authorizationServerUrl, authorizationServerUrl)
  ) {
    throw new OAuthClientFlowError(
      "stored OAuth discovery does not match its authorization server",
    );
  }
  const issuer = canonicalIssuer(
    discovery.authorizationServerMetadata?.issuer ?? authorizationServerUrl,
  );
  if (!sameIssuer(authorizationServerUrl, issuer)) {
    throw new OAuthClientFlowError(
      "stored OAuth metadata belongs to a different authorization server",
    );
  }
  const resourceUrl = provider.resourceUrl();
  return {
    authorizationServerUrl,
    issuer,
    metadata: discovery.authorizationServerMetadata,
    resource: resourceUrl === undefined ? undefined : new URL(resourceUrl),
  };
}

function providerFor(request: OAuthFlowRequest): {
  readonly provider: RenoaOAuthProvider;
  readonly scope?: string;
  readonly forceReauthorization: boolean;
} {
  if (request.action === "oauth_begin") {
    const retained = RenoaOAuthProvider.begin(
      request.oauth_state,
      request.csrf_state,
      request.redirect_uri,
      false,
      request.endpoint,
      request.registration,
    );
    const upgrade = scopeUpgrade(
      retained.grantedScope(),
      request.requested_scope,
    );
    const forceReauthorization =
      request.force_reauthorization || upgrade.widensGrant;
    return {
      provider: RenoaOAuthProvider.begin(
        request.oauth_state,
        request.csrf_state,
        request.redirect_uri,
        forceReauthorization,
        request.endpoint,
        request.registration,
      ),
      ...(upgrade.scope === undefined ? {} : { scope: upgrade.scope }),
      forceReauthorization,
    };
  }
  return {
    provider: new RenoaOAuthProvider(
      request.oauth_state,
      request.endpoint,
      request.action === "oauth_token"
        ? { mode: "dynamic" }
        : request.registration,
    ),
    forceReauthorization: false,
  };
}

function authorized(
  provider: RenoaOAuthProvider,
  accessToken: string,
): AdapterRecord {
  return {
    wire_version: WIRE_VERSION,
    event: "oauth_authorized",
    authorization: { scheme: "bearer", token: accessToken },
    oauth_state: provider.snapshot(),
  };
}

function oauthFailure(
  error: unknown,
  tracker: OAuthExchangeTracker,
  cancelled: boolean,
): WireFailure {
  if (
    error instanceof Error &&
    (error.message ===
      "Incompatible auth server: does not support dynamic client registration" ||
      error.message ===
        "OAuth client information must be saveable for dynamic registration")
  ) {
    return {
      kind: "protocol",
      certainty: "definite",
      message:
        "The authorization server does not support the selected OAuth client registration mode.",
      partial_changes_possible: false,
      diagnostic: {
        code: "oauth_registration_required",
        detail:
          "Configure this connection with pre_registered OAuth credentials or a Client ID Metadata Document URL supported by the authorization server.",
      },
    };
  }
  if (error instanceof RegistrationRejectedError) {
    return {
      kind: "protocol",
      certainty: "definite",
      message: `OAuth client registration was rejected with HTTP ${error.status}.`,
      partial_changes_possible: true,
      diagnostic: {
        code: registrationErrorCode(error.body) ?? "registration_rejected",
        http_status: error.status,
        detail: "The authorization server returned a Dynamic Client Registration error.",
      },
    };
  }
  if (error instanceof IssuerMismatchError) {
    return {
      kind: "protocol",
      certainty: "definite",
      message: "OAuth callback issuer validation failed; the authorization code was not sent.",
      partial_changes_possible: false,
      diagnostic: {
        code: "oauth_issuer_mismatch",
        detail: "The callback issuer did not match the validated authorization server.",
      },
    };
  }
  if (error instanceof OAuthError) {
    const httpStatus = tracker.responseStatus();
    return {
      kind: "protocol",
      certainty: "definite",
      message: `OAuth server rejected the credential request with '${error.code}'.`,
      partial_changes_possible: tracker.evidence().dispatchStarted,
      diagnostic: {
        code: error.code,
        ...(httpStatus === undefined ? {} : { http_status: httpStatus }),
        detail: safeOAuthDetail(error.message),
      },
    };
  }
  if (error instanceof OAuthClientFlowError) {
    return {
      kind: "protocol",
      certainty: "definite",
      message: "OAuth security validation rejected the authorization flow.",
      partial_changes_possible: tracker.evidence().dispatchStarted,
      diagnostic: {
        code: "oauth_flow_rejected",
        detail: "The pinned MCP OAuth client rejected the flow before it could continue safely.",
      },
    };
  }
  const failure = toWireFailure(error, tracker.evidence(), cancelled);
  return {
    ...failure,
    message: failure.message
      .replaceAll("MCP server", "OAuth server")
      .replaceAll("MCP endpoint", "OAuth endpoint")
      .replaceAll("MCP response", "OAuth response")
      .replaceAll("tool call", "credential request")
      .replaceAll("tool outcome", "credential outcome"),
  };
}

function safeOAuthDetail(value: string): string {
  const detail = [...value]
    .filter((character) => character === "\n" || !/[\u0000-\u001F\u007F]/u.test(character))
    .slice(0, 1_024)
    .join("");
  return detail.length === 0
    ? "The authorization server returned a standard OAuth error."
    : detail;
}

function registrationErrorCode(body: string): string | undefined {
  try {
    const value = JSON.parse(body) as unknown;
    if (typeof value !== "object" || value === null || Array.isArray(value)) {
      return undefined;
    }
    const code = (value as { readonly error?: unknown }).error;
    return typeof code === "string" && /^[a-z][a-z0-9_]{0,127}$/u.test(code)
      ? code
      : undefined;
  } catch {
    return undefined;
  }
}

function redactExactSecrets(
  failure: WireFailure,
  exactSecrets: readonly string[],
): WireFailure {
  let message = failure.message;
  let code = failure.diagnostic.code;
  let detail = failure.diagnostic.detail;
  for (const secret of exactSecrets) {
    if (secret.length === 0) continue;
    message = message.replaceAll(secret, "[REDACTED]");
    code = code?.replaceAll(secret, "[REDACTED]");
    detail = detail.replaceAll(secret, "[REDACTED]");
  }
  return {
    ...failure,
    message,
    diagnostic: {
      ...failure.diagnostic,
      ...(code === undefined ? {} : { code }),
      detail,
    },
  };
}

export function oauthSecrets(state: WireOAuthState): string[] {
  const secrets: string[] = [];
  const pending: Array<{ readonly key: string; readonly value: unknown }> = [
    { key: "oauth_state", value: state },
  ];
  while (pending.length > 0) {
    const current = pending.pop();
    if (current === undefined) continue;
    if (Array.isArray(current.value)) {
      for (const value of current.value) pending.push({ key: current.key, value });
      continue;
    }
    if (typeof current.value !== "object" || current.value === null) continue;
    for (const [key, value] of Object.entries(current.value)) {
      if (
        typeof value === "string" &&
        /(?:token|secret|verifier|csrf_state|authorization_code)/iu.test(key)
      ) {
        secrets.push(value);
      } else {
        pending.push({ key, value });
      }
    }
  }
  return secrets;
}
