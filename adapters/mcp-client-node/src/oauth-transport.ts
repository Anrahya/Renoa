import type { FetchLike } from "@modelcontextprotocol/client";
import { isLoopbackHost } from "./endpoint.js";
import { AdapterProblem, type ExchangeEvidence } from "./errors.js";
import { MAX_HTTP_RESPONSE_BYTES } from "./limits.js";

export class OAuthExchangeTracker {
  #postCount = 0;
  #postResponseStarted = false;

  markRequest(method: string): void {
    if (method !== "POST") {
      return;
    }
    if (this.#postCount !== 0) {
      throw new AdapterProblem(
        "protocol",
        "MCP OAuth SDK attempted a hidden credential side-effect retry.",
        {
          code: "hidden_oauth_retry_blocked",
          partialChangesPossible: true,
        },
      );
    }
    this.#postCount += 1;
    this.#postResponseStarted = false;
  }

  markResponse(method: string): void {
    if (method === "POST") {
      this.#postResponseStarted = true;
    }
  }

  evidence(): ExchangeEvidence {
    return {
      dispatchStarted: this.#postCount > 0,
      responseStarted: this.#postResponseStarted,
    };
  }
}

export function guardedOAuthFetch(
  tracker: OAuthExchangeTracker,
  operationSignal: AbortSignal,
): FetchLike {
  return async (input, init): Promise<Response> => {
    const url = new URL(input instanceof Request ? input.url : input);
    validateOAuthUrl(url);
    const method = (init?.method ?? (input instanceof Request ? input.method : "GET"))
      .toUpperCase();
    tracker.markRequest(method);
    const signal = init?.signal == null
      ? operationSignal
      : AbortSignal.any([operationSignal, init.signal]);
    const response = await globalThis.fetch(input, {
      ...init,
      redirect: "manual",
      signal,
    });
    tracker.markResponse(method);
    if (response.status >= 300 && response.status < 400) {
      throw new AdapterProblem(
        "protocol",
        "OAuth endpoint redirect was blocked; metadata must name the final endpoint.",
        {
          code: "oauth_redirect_blocked",
          httpStatus: response.status,
          partialChangesPossible: method === "POST",
        },
      );
    }
    return boundResponse(response, method === "POST");
  };
}

function validateOAuthUrl(url: URL): void {
  const loopback =
    url.protocol === "http:" &&
    isLoopbackHost(url.hostname);
  if (
    (url.protocol !== "https:" && !loopback) ||
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.hash.length > 0
  ) {
    throw new AdapterProblem(
      "invalid_endpoint",
      "OAuth metadata selected an insecure or malformed URL.",
      { code: "invalid_oauth_url" },
    );
  }
}

function boundResponse(response: Response, sideEffect: boolean): Response {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (!Number.isSafeInteger(parsed) || parsed < 0) {
      throw new AdapterProblem(
        "protocol",
        "OAuth response has an invalid Content-Length.",
        {
          code: "invalid_content_length",
          httpStatus: response.status,
          partialChangesPossible: sideEffect,
        },
      );
    }
    if (parsed > MAX_HTTP_RESPONSE_BYTES) {
      throw responseLimit(response.status, sideEffect);
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
        throw responseLimit(response.status, sideEffect);
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

function responseLimit(status: number, sideEffect: boolean): AdapterProblem {
  return new AdapterProblem(
    "resource_limit",
    `OAuth response exceeds ${MAX_HTTP_RESPONSE_BYTES} bytes.`,
    {
      code: "oauth_response_limit",
      httpStatus: status,
      partialChangesPossible: sideEffect,
    },
  );
}
