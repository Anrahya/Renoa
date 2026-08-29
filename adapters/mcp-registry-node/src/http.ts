import { RegistryProblem } from "./errors.js";
import { MAX_RESPONSE_BYTES } from "./limits.js";

export interface RequestContext {
  readonly baseUrl: URL;
  readonly signal: AbortSignal;
  readonly parentSignal?: AbortSignal;
  readonly timeoutSignal: AbortSignal;
}

export function registryBaseUrl(value: string): URL {
  let url: URL;
  try {
    url = new URL(value);
  } catch (error) {
    throw new RegistryProblem(
      "invalid_request",
      "MCP Registry base URL is invalid.",
      { code: "invalid_registry_base_url", cause: error },
    );
  }
  const loopback =
    url.hostname === "127.0.0.1" ||
    url.hostname === "[::1]" ||
    url.hostname === "localhost";
  if (
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.search.length > 0 ||
    url.hash.length > 0 ||
    url.pathname !== "/" ||
    (url.protocol !== "https:" && !(url.protocol === "http:" && loopback))
  ) {
    throw new RegistryProblem(
      "invalid_request",
      "MCP Registry base URL must be HTTPS at the origin root, or HTTP loopback for tests.",
      { code: "invalid_registry_base_url" },
    );
  }
  return url;
}

export async function getJson(
  path: string,
  context: RequestContext,
): Promise<unknown> {
  const url = new URL(path, context.baseUrl);
  let response: Response;
  try {
    response = await fetch(url, {
      method: "GET",
      headers: { accept: "application/json" },
      redirect: "error",
      signal: context.signal,
    });
  } catch (error) {
    if (context.parentSignal?.aborted === true) {
      throw new RegistryProblem(
        "cancelled",
        "Official MCP Registry discovery was cancelled.",
        { code: "registry_cancelled", cause: error },
      );
    }
    if (context.timeoutSignal.aborted) {
      throw new RegistryProblem(
        "timeout",
        "Official MCP Registry discovery timed out without returning a result.",
        { code: "registry_timeout", cause: error },
      );
    }
    throw new RegistryProblem(
      "unavailable",
      "Official MCP Registry discovery is unavailable.",
      { code: "registry_unavailable", cause: error },
    );
  }
  if (!response.ok) {
    await response.body?.cancel();
    const kind =
      response.status === 404
        ? "not_found"
        : response.status === 429 || response.status >= 500
          ? "unavailable"
          : "protocol";
    throw new RegistryProblem(
      kind,
      `Official MCP Registry returned HTTP ${response.status}; no extension was installed.`,
      {
        code: "registry_http_error",
        httpStatus: response.status,
      },
    );
  }
  const bytes = await readBounded(response);
  try {
    const text = new TextDecoder("utf-8", { fatal: true }).decode(bytes);
    return JSON.parse(text) as unknown;
  } catch (error) {
    throw new RegistryProblem(
      "protocol",
      "Official MCP Registry returned malformed JSON.",
      { code: "invalid_registry_json", cause: error },
    );
  }
}

async function readBounded(response: Response): Promise<Uint8Array> {
  const declared = response.headers.get("content-length");
  if (declared !== null) {
    const bytes = Number(declared);
    if (Number.isFinite(bytes) && bytes > MAX_RESPONSE_BYTES) {
      await response.body?.cancel();
      throw responseLimit();
    }
  }
  if (response.body === null) {
    throw new RegistryProblem(
      "protocol",
      "Official MCP Registry returned an empty response body.",
      { code: "empty_registry_response" },
    );
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  while (true) {
    const item = await reader.read();
    if (item.done) {
      break;
    }
    length += item.value.byteLength;
    if (length > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw responseLimit();
    }
    chunks.push(item.value);
  }
  const output = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return output;
}

function responseLimit(): RegistryProblem {
  return new RegistryProblem(
    "resource_limit",
    `Official MCP Registry response exceeds ${MAX_RESPONSE_BYTES} bytes.`,
    { code: "registry_response_limit" },
  );
}
