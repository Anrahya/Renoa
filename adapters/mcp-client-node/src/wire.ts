import type {
  AdapterRequest,
  FrozenMcpTool,
  WireCredential,
  WireHeaders,
} from "./contract.js";
import { AdapterProblem } from "./errors.js";
import {
  MAX_AUTH_TOKEN_BYTES,
  MAX_CREDENTIAL_PREFIX_BYTES,
  MAX_OAUTH_SCOPE_BYTES,
  MAX_OAUTH_STATE_BYTES,
  MAX_OAUTH_VALUE_BYTES,
  MAX_REQUEST_HEADER_BYTES,
  MAX_REQUEST_HEADERS,
  WIRE_VERSION,
} from "./limits.js";
import { parseOAuthRegistration } from "./oauth-registration-wire.js";
import { isValidOAuthScope } from "./oauth-scope.js";
import {
  invalid,
  requireBoolean,
  requireBoundedString,
  requireExactKeys,
  requireJsonObject,
  requireObject,
  requireString,
} from "./wire-values.js";

const HEADER_TOKEN = /^[!#$%&'*+\-.^_`|~0-9A-Za-z]+$/;
const CLIENT_OWNED_HEADERS = new Set([
  "accept",
  "authorization",
  "connection",
  "content-length",
  "content-type",
  "cookie",
  "host",
  "keep-alive",
  "mcp-method",
  "mcp-protocol-version",
  "mcp-session-id",
  "proxy-authorization",
  "set-cookie",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
  "x-api-key",
]);

const FORBIDDEN_CREDENTIAL_HEADERS = new Set([
  "accept",
  "connection",
  "content-length",
  "content-type",
  "host",
  "keep-alive",
  "mcp-method",
  "mcp-protocol-version",
  "mcp-session-id",
  "proxy-authorization",
  "set-cookie",
  "te",
  "trailer",
  "transfer-encoding",
  "upgrade",
]);

export function parseAdapterRequest(value: unknown): AdapterRequest {
  const request = requireObject(value, "request");
  if (request.wire_version !== WIRE_VERSION) {
    throw invalid(`request.wire_version must be ${WIRE_VERSION}`);
  }
  if (request.action === "discover") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint", "headers", "credential"],
      "request",
      ["headers", "credential"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "discover",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      ...optionalHeaders(request.headers),
      ...optionalCredential(request.credential),
    };
  }
  if (request.action === "call") {
    requireExactKeys(
      request,
      [
        "wire_version",
        "action",
        "endpoint",
        "protocol_version",
        "headers",
        "credential",
        "tool",
        "arguments",
      ],
      "request",
      ["headers", "credential"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "call",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      protocol_version: requireString(
        request.protocol_version,
        "request.protocol_version",
      ),
      ...optionalHeaders(request.headers),
      ...optionalCredential(request.credential),
      tool: parseFrozenTool(request.tool),
      arguments: requireJsonObject(request.arguments, "request.arguments"),
    };
  }
  if (request.action === "oauth_discover") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint"],
      "request",
    );
    return {
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: requireString(request.endpoint, "request.endpoint"),
    };
  }
  if (request.action === "oauth_begin") {
    requireExactKeys(
      request,
      [
        "wire_version",
        "action",
        "endpoint",
        "csrf_state",
        "redirect_uri",
        "force_reauthorization",
        "requested_scope",
        "registration",
        "oauth_state",
      ],
      "request",
      ["requested_scope", "oauth_state"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "oauth_begin",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      csrf_state: requireBoundedString(
        request.csrf_state,
        "request.csrf_state",
        MAX_OAUTH_VALUE_BYTES,
      ),
      redirect_uri: requireBoundedString(
        request.redirect_uri,
        "request.redirect_uri",
        MAX_OAUTH_VALUE_BYTES,
      ),
      force_reauthorization: requireBoolean(
        request.force_reauthorization,
        "request.force_reauthorization",
      ),
      ...(request.requested_scope === undefined
        ? {}
        : {
            requested_scope: requireOAuthScope(request.requested_scope),
          }),
      registration: parseOAuthRegistration(request.registration),
      ...(request.oauth_state === undefined
        ? {}
        : { oauth_state: parseOAuthState(request.oauth_state) }),
    };
  }
  if (request.action === "oauth_exchange") {
    requireExactKeys(
      request,
      [
        "wire_version",
        "action",
        "endpoint",
        "authorization_code",
        "issuer",
        "registration",
        "oauth_state",
      ],
      "request",
      ["issuer"],
    );
    return {
      wire_version: WIRE_VERSION,
      action: "oauth_exchange",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      authorization_code: requireBoundedString(
        request.authorization_code,
        "request.authorization_code",
        MAX_OAUTH_VALUE_BYTES,
      ),
      ...(request.issuer === undefined
        ? {}
        : {
            issuer: requireBoundedString(
              request.issuer,
              "request.issuer",
              MAX_OAUTH_VALUE_BYTES,
            ),
          }),
      registration: parseOAuthRegistration(request.registration),
      oauth_state: parseOAuthState(request.oauth_state),
    };
  }
  if (request.action === "oauth_token") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint", "oauth_state"],
      "request",
    );
    return {
      wire_version: WIRE_VERSION,
      action: "oauth_token",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      oauth_state: parseOAuthState(request.oauth_state),
    };
  }
  if (request.action === "oauth_refresh") {
    requireExactKeys(
      request,
      ["wire_version", "action", "endpoint", "registration", "oauth_state"],
      "request",
    );
    return {
      wire_version: WIRE_VERSION,
      action: "oauth_refresh",
      endpoint: requireString(request.endpoint, "request.endpoint"),
      registration: parseOAuthRegistration(request.registration),
      oauth_state: parseOAuthState(request.oauth_state),
    };
  }
  throw invalid(
    "request.action must be 'discover', 'call', 'oauth_discover', 'oauth_begin', 'oauth_exchange', 'oauth_token', or 'oauth_refresh'",
  );
}

function requireOAuthScope(value: unknown): string {
  const scope = requireBoundedString(
    value,
    "request.requested_scope",
    MAX_OAUTH_SCOPE_BYTES,
  );
  if (!isValidOAuthScope(scope)) {
    throw invalid(
      "request.requested_scope must contain single-space-delimited RFC 6749 scope tokens",
    );
  }
  return scope;
}

function parseOAuthState(value: unknown): import("./contract.js").WireOAuthState {
  const state = requireObject(value, "request.oauth_state");
  requireExactKeys(
    state,
    [
      "schema_version",
      "mcp_endpoint",
      "csrf_state",
      "redirect_uri",
      "authorization_url",
      "authorization_server_url",
      "client_information",
      "code_verifier",
      "discovery_state",
      "resource_url",
      "oauth_scope",
      "tokens",
      "tokens_saved_at_ms",
    ],
    "request.oauth_state",
    [
      "authorization_url",
      "authorization_server_url",
      "client_information",
      "code_verifier",
      "discovery_state",
      "resource_url",
      "oauth_scope",
      "tokens",
      "tokens_saved_at_ms",
    ],
  );
  let encoded: string;
  try {
    encoded = JSON.stringify(state);
  } catch (error) {
    throw new AdapterProblem(
      "invalid_request",
      "request.oauth_state is not valid JSON",
      { code: "invalid_wire_request", cause: error },
    );
  }
  if (Buffer.byteLength(encoded, "utf8") > MAX_OAUTH_STATE_BYTES) {
    throw invalid(`request.oauth_state exceeds ${MAX_OAUTH_STATE_BYTES} bytes`);
  }
  if (state.schema_version !== 1) {
    throw invalid("request.oauth_state.schema_version must be 1");
  }
  requireBoundedString(
    state.mcp_endpoint,
    "request.oauth_state.mcp_endpoint",
    MAX_OAUTH_VALUE_BYTES,
  );
  requireBoundedString(
    state.csrf_state,
    "request.oauth_state.csrf_state",
    MAX_OAUTH_VALUE_BYTES,
  );
  requireBoundedString(
    state.redirect_uri,
    "request.oauth_state.redirect_uri",
    MAX_OAUTH_VALUE_BYTES,
  );
  return state as import("./contract.js").WireOAuthState;
}

function optionalHeaders(
  value: unknown,
): { readonly headers?: WireHeaders } {
  if (value === undefined) {
    return {};
  }
  const input = requireObject(value, "request.headers");
  const entries = Object.entries(input);
  if (entries.length > MAX_REQUEST_HEADERS) {
    throw invalid(`request.headers exceeds ${MAX_REQUEST_HEADERS} entries`);
  }
  const normalized = new Map<string, string>();
  let bytes = 0;
  for (const [name, unknownValue] of entries) {
    const lower = name.toLowerCase();
    if (!HEADER_TOKEN.test(name) || CLIENT_OWNED_HEADERS.has(lower)) {
      throw invalid(`request.headers contains forbidden name '${name}'`);
    }
    if (normalized.has(lower)) {
      throw invalid(`request.headers repeats '${name}' case-insensitively`);
    }
    if (typeof unknownValue !== "string") {
      throw invalid(`request.headers.${name} must be a string`);
    }
    try {
      new Headers([[name, unknownValue]]);
    } catch (error) {
      throw new AdapterProblem(
        "invalid_request",
        `request.headers.${name} is not a valid HTTP header value`,
        { code: "invalid_wire_request", cause: error },
      );
    }
    bytes += Buffer.byteLength(name, "utf8") + Buffer.byteLength(unknownValue, "utf8");
    if (bytes > MAX_REQUEST_HEADER_BYTES) {
      throw invalid(`request.headers exceeds ${MAX_REQUEST_HEADER_BYTES} bytes`);
    }
    normalized.set(lower, unknownValue);
  }
  return { headers: Object.fromEntries(normalized) };
}

function optionalCredential(
  value: unknown,
): { readonly credential?: WireCredential } {
  if (value === undefined) {
    return {};
  }
  const credential = requireObject(value, "request.credential");
  requireExactKeys(
    credential,
    ["scheme", "name", "prefix", "secret"],
    "request.credential",
  );
  if (credential.scheme !== "header") {
    throw invalid("request.credential.scheme must be 'header'");
  }
  const name = requireString(credential.name, "request.credential.name");
  const lower = name.toLowerCase();
  if (!HEADER_TOKEN.test(name) || FORBIDDEN_CREDENTIAL_HEADERS.has(lower)) {
    throw invalid("request.credential.name is not an allowed credential header");
  }
  if (typeof credential.prefix !== "string") {
    throw invalid("request.credential.prefix must be a string");
  }
  const prefix = credential.prefix;
  if (
    Buffer.byteLength(prefix, "utf8") > MAX_CREDENTIAL_PREFIX_BYTES ||
    /[^\t\x20-\x7E]/u.test(prefix)
  ) {
    throw invalid("request.credential.prefix is malformed or over limit");
  }
  const secret = requireString(credential.secret, "request.credential.secret");
  if (
    Buffer.byteLength(secret, "utf8") > MAX_AUTH_TOKEN_BYTES ||
    /[\s\u0000-\u001F\u007F]/u.test(secret)
  ) {
    throw invalid("request.credential.secret is malformed or over limit");
  }
  try {
    new Headers([[name, `${prefix}${secret}`]]);
  } catch (error) {
    throw new AdapterProblem(
      "invalid_request",
      "request.credential does not form a valid HTTP header",
      { code: "invalid_wire_request", cause: error },
    );
  }
  return {
    credential: { scheme: "header", name: lower, prefix, secret },
  };
}

function parseFrozenTool(value: unknown): FrozenMcpTool {
  const tool = requireObject(value, "request.tool");
  requireExactKeys(
    tool,
    ["name", "input_schema", "output_schema"],
    "request.tool",
    ["output_schema"],
  );
  return {
    name: requireString(tool.name, "request.tool.name"),
    input_schema: requireJsonObject(
      tool.input_schema,
      "request.tool.input_schema",
    ),
    ...(tool.output_schema === undefined
      ? {}
      : {
          output_schema: requireJsonObject(
            tool.output_schema,
            "request.tool.output_schema",
          ),
        }),
  };
}
