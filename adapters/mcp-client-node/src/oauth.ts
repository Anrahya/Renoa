import {
  auth,
  IssuerMismatchError,
  OAuthClientFlowError,
  OAuthError,
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
import { RenoaOAuthProvider } from "./oauth-state.js";
import { guardedOAuthFetch, OAuthExchangeTracker } from "./oauth-transport.js";

type OAuthRequest = Exclude<
  AdapterRequest,
  { readonly action: "discover" | "call" }
>;

export function isOAuthRequest(request: AdapterRequest): request is OAuthRequest {
  return request.action.startsWith("oauth_");
}

export async function executeOAuthRequest(
  request: OAuthRequest,
  signal: AbortSignal,
): Promise<AdapterRecord> {
  const provider = providerFor(request);
  const tracker = new OAuthExchangeTracker();
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
    const result = await auth(provider, {
      serverUrl: request.endpoint,
      fetchFn: guardedOAuthFetch(tracker, signal),
      ...(request.action === "oauth_exchange"
        ? {
            authorizationCode: request.authorization_code,
            ...(request.issuer === undefined ? {} : { iss: request.issuer }),
          }
        : {}),
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

function providerFor(request: OAuthRequest): RenoaOAuthProvider {
  if (request.action === "oauth_begin") {
    return RenoaOAuthProvider.begin(
      request.oauth_state,
      request.csrf_state,
      request.redirect_uri,
      request.force_reauthorization,
      request.endpoint,
    );
  }
  return new RenoaOAuthProvider(request.oauth_state, request.endpoint);
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
    return {
      kind: "protocol",
      certainty: "definite",
      message: `OAuth server rejected the credential request with '${error.code}'.`,
      partial_changes_possible: tracker.evidence().dispatchStarted,
      diagnostic: {
        code: error.code,
        detail: "The authorization server returned a standard OAuth error.",
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
