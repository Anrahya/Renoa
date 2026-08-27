import type { FetchLike } from "@modelcontextprotocol/client";
import type { AdapterRequest, WireAuthorization } from "./contract.js";
import { AdapterProblem, type ExchangeEvidence } from "./errors.js";
import { MAX_HTTP_RESPONSE_BYTES } from "./limits.js";

export class CallExchangeTracker {
  callRequestCount = 0;
  dispatchStarted = false;
  responseStarted = false;
  boundaryProblem: AdapterProblem | undefined;

  markDispatched(): void {
    this.callRequestCount += 1;
    this.dispatchStarted = true;
  }

  markResponseStarted(): void {
    this.responseStarted = true;
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
  authorization: WireAuthorization | undefined,
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
    const headers = new Headers(input instanceof Request ? input.headers : undefined);
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
    if (authorization !== undefined) {
      headers.set("authorization", `Bearer ${authorization.token}`);
    }
    const requestMethod = (init?.method ?? "GET").toUpperCase();
    const mcpMethod = headers.get("mcp-method");
    if (requestMethod !== "POST" || mcpMethod === null) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP adapter attempted a transport operation outside stateless POST requests.",
          {
            code: "unexpected_transport_request",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    const allowed =
      mcpMethod === "server/discover" ||
      (action === "discover" && mcpMethod === "tools/list") ||
      (action === "call" && mcpMethod === "tools/call");
    if (!allowed) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          `MCP adapter attempted unexpected method '${mcpMethod}'.`,
          {
            code: "unexpected_mcp_method",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }

    const isToolCall = mcpMethod === "tools/call";
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
    if (isToolCall) {
      tracker.markResponseStarted();
    }
    if (response.headers.has("mcp-session-id")) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP endpoint attempted to create a session; Renoa v0 is stateless.",
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
