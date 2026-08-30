import type { FetchLike } from "@modelcontextprotocol/client";
import type {
  AdapterRequest,
  WireCredential,
  WireHeaders,
} from "./contract.js";
import { AdapterProblem, type ExchangeEvidence } from "./errors.js";
import { MAX_HTTP_RESPONSE_BYTES } from "./limits.js";

export class CallExchangeTracker {
  callRequestCount = 0;
  dispatchStarted = false;
  responseStarted = false;
  legacyInitialized = false;
  legacyStreamRequestCount = 0;
  legacySessionId: string | undefined;
  boundaryProblem: AdapterProblem | undefined;

  markDispatched(): void {
    this.callRequestCount += 1;
    this.dispatchStarted = true;
  }

  markResponseStarted(): void {
    this.responseStarted = true;
  }

  markLegacyInitialized(): void {
    this.legacyInitialized = true;
  }

  markLegacyStreamRequest(): void {
    this.legacyStreamRequestCount += 1;
  }

  recordLegacySession(sessionId: string | null): void {
    this.legacySessionId = sessionId ?? undefined;
  }

  recordBoundaryProblem(problem: AdapterProblem): void {
    this.boundaryProblem ??= problem;
  }

  reject(problem: AdapterProblem): never {
    this.recordBoundaryProblem(problem);
    throw problem;
  }

  evidence(): ExchangeEvidence {
    return {
      dispatchStarted: this.dispatchStarted,
      responseStarted: this.responseStarted,
    };
  }
}

export class Deadline {
  readonly expiresAt: number;

  constructor(durationMs: number) {
    this.expiresAt = Date.now() + durationMs;
  }

  requestOptions(signal: AbortSignal) {
    const remaining = this.expiresAt - Date.now();
    if (remaining <= 0) {
      throw new AdapterProblem(
        "timeout",
        "MCP operation exceeded its total deadline.",
        { code: "total_deadline" },
      );
    }
    return {
      signal,
      timeout: remaining,
      maxTotalTimeout: remaining,
      resetTimeoutOnProgress: false,
    } as const;
  }
}

export function guardedFetch(
  action: AdapterRequest["action"],
  tracker: CallExchangeTracker,
  dispatchStarted: () => Promise<void>,
  operationSignal: AbortSignal,
  endpoint: URL,
  configuredHeaders: WireHeaders | undefined,
  credential: WireCredential | undefined,
): FetchLike {
  const nativeFetch = globalThis.fetch;
  return async (input, init): Promise<Response> => {
    const requestUrl = new URL(input instanceof Request ? input.url : input);
    if (requestUrl.href !== endpoint.href) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP adapter blocked a request outside the configured endpoint.",
          {
            code: "credential_scope_violation",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    const headers = new Headers(configuredHeaders);
    new Headers(input instanceof Request ? input.headers : undefined).forEach(
      (value, name) => headers.set(name, value),
    );
    new Headers(init?.headers).forEach((value, name) => headers.set(name, value));
    if (headers.has("authorization")) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP SDK attempted to provide its own Authorization header.",
          {
            code: "unexpected_authorization_header",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    if (credential !== undefined) {
      if (headers.has(credential.name)) {
        tracker.reject(
          new AdapterProblem(
            "protocol",
            `MCP SDK or public configuration attempted to provide credential header '${credential.name}'.`,
            {
              code: "unexpected_credential_header",
              partialChangesPossible: tracker.dispatchStarted,
            },
          ),
        );
      }
      headers.set(
        credential.name,
        `${credential.prefix}${credential.secret}`,
      );
    }
    const requestMethod = (init?.method ?? "GET").toUpperCase();
    const modernMethod = headers.get("mcp-method");
    if (requestMethod === "GET") {
      if (
        !tracker.legacyInitialized ||
        tracker.legacyStreamRequestCount !== 0
      ) {
        tracker.reject(
          new AdapterProblem(
            "protocol",
            "MCP adapter attempted an unexpected legacy event stream.",
            {
              code: "unexpected_transport_request",
              partialChangesPossible: tracker.dispatchStarted,
            },
          ),
        );
      }
      requireLegacySessionHeader(headers, tracker);
      tracker.markLegacyStreamRequest();
      return boundResponse(
        await nativeFetch(input, {
          ...init,
          headers,
          redirect: "manual",
          signal:
            init?.signal == null
              ? operationSignal
              : AbortSignal.any([operationSignal, init.signal]),
        }),
        false,
        tracker,
      );
    }
    if (requestMethod !== "POST") {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP adapter attempted an unsupported transport request.",
          {
            code: "unexpected_transport_request",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    const bodyMethod = rpcMethod(init?.body, tracker);
    if (modernMethod !== null && modernMethod !== bodyMethod) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP method header does not match the JSON-RPC request body.",
          {
            code: "mcp_method_mismatch",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    const legacy = modernMethod === null;
    const allowed = legacy
      ? bodyMethod === "initialize" ||
        bodyMethod === "notifications/initialized" ||
        (action === "discover" && bodyMethod === "tools/list") ||
        (action === "call" && bodyMethod === "tools/call")
      : bodyMethod === "server/discover" ||
        (action === "discover" && bodyMethod === "tools/list") ||
        (action === "call" && bodyMethod === "tools/call");
    if (!allowed) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          `MCP adapter attempted unexpected method '${bodyMethod}'.`,
          {
            code: "unexpected_mcp_method",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }

    if (legacy) {
      if (bodyMethod === "initialize") {
        if (headers.has("mcp-session-id")) {
          tracker.reject(
            new AdapterProblem(
              "protocol",
              "Legacy MCP initialize unexpectedly carried a session id.",
              { code: "unexpected_session_id" },
            ),
          );
        }
      } else {
        requireLegacySessionHeader(headers, tracker);
      }
      if (bodyMethod === "notifications/initialized") {
        tracker.markLegacyInitialized();
      }
    }

    const isToolCall = bodyMethod === "tools/call";
    if (isToolCall) {
      if (tracker.callRequestCount !== 0) {
        tracker.reject(
          new AdapterProblem(
            "protocol",
            "MCP SDK attempted a hidden tools/call retry; Renoa blocked it.",
            { code: "hidden_retry_blocked", partialChangesPossible: true },
          ),
        );
      }
      await dispatchStarted();
      tracker.markDispatched();
    }

    const signal =
      init?.signal == null
        ? operationSignal
        : AbortSignal.any([operationSignal, init.signal]);
    const response = await nativeFetch(input, {
      ...init,
      headers,
      redirect: "manual",
      signal,
    });
    if (!isToolCall && response.status >= 300 && response.status < 400) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP endpoint redirects are not followed across the credential boundary.",
          {
            code: "redirect_blocked",
            partialChangesPossible: isToolCall,
            httpStatus: response.status,
          },
        ),
      );
    }
    if (isToolCall) {
      tracker.markResponseStarted();
    }
    if (legacy && bodyMethod === "initialize" && response.ok) {
      tracker.recordLegacySession(response.headers.get("mcp-session-id"));
    } else if (!legacy && response.headers.has("mcp-session-id")) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "Modern MCP endpoint attempted to create a legacy session.",
          {
            code: "session_not_supported",
            partialChangesPossible: isToolCall,
            httpStatus: response.status,
          },
        ),
      );
    }
    return boundResponse(response, isToolCall, tracker);
  };
}

function rpcMethod(
  body: BodyInit | null | undefined,
  tracker: CallExchangeTracker,
): string {
  if (typeof body !== "string") {
    return tracker.reject(
      new AdapterProblem(
        "protocol",
        "MCP SDK emitted a POST body Renoa could not inspect.",
        {
          code: "uninspectable_request_body",
          partialChangesPossible: tracker.dispatchStarted,
        },
      ),
    );
  }
  let value: unknown;
  try {
    value = JSON.parse(body) as unknown;
  } catch (error) {
    return tracker.reject(
      new AdapterProblem(
        "protocol",
        "MCP SDK emitted an invalid JSON request body.",
        {
          code: "invalid_sdk_request_body",
          partialChangesPossible: tracker.dispatchStarted,
          cause: error,
        },
      ),
    );
  }
  if (
    typeof value !== "object" ||
    value === null ||
    Array.isArray(value) ||
    typeof (value as { readonly method?: unknown }).method !== "string"
  ) {
    return tracker.reject(
      new AdapterProblem(
        "protocol",
        "MCP SDK emitted a request without one JSON-RPC method.",
        {
          code: "invalid_sdk_request_shape",
          partialChangesPossible: tracker.dispatchStarted,
        },
      ),
    );
  }
  return (value as { readonly method: string }).method;
}

function requireLegacySessionHeader(
  headers: Headers,
  tracker: CallExchangeTracker,
): void {
  const observed = headers.get("mcp-session-id") ?? undefined;
  if (observed !== tracker.legacySessionId) {
    tracker.reject(
      new AdapterProblem(
        "protocol",
        "MCP SDK changed or invented the negotiated legacy session id.",
        {
          code: "session_id_mismatch",
          partialChangesPossible: tracker.dispatchStarted,
        },
      ),
    );
  }
}

function boundResponse(
  response: Response,
  toolCall: boolean,
  tracker: CallExchangeTracker,
): Response {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (!Number.isSafeInteger(parsed) || parsed < 0) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP response has an invalid Content-Length.",
          {
            code: "invalid_content_length",
            partialChangesPossible: toolCall,
            httpStatus: response.status,
          },
        ),
      );
    }
    if (parsed > MAX_HTTP_RESPONSE_BYTES) {
      tracker.reject(responseLimit(toolCall, response.status));
    }
  }
  if (response.body === null) {
    return response;
  }

  let bytes = 0;
  const limiter = new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      bytes += chunk.byteLength;
      if (bytes > MAX_HTTP_RESPONSE_BYTES) {
        const problem = responseLimit(toolCall, response.status);
        tracker.recordBoundaryProblem(problem);
        throw problem;
      }
      controller.enqueue(chunk);
    },
  });
  return new Response(response.body.pipeThrough(limiter), {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}

function responseLimit(toolCall: boolean, status: number): AdapterProblem {
  return new AdapterProblem(
    "resource_limit",
    `MCP response exceeds ${MAX_HTTP_RESPONSE_BYTES} bytes.`,
    {
      code: "http_response_limit",
      partialChangesPossible: toolCall,
      httpStatus: status,
    },
  );
}
