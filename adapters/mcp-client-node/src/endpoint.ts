import { AdapterProblem } from "./errors.js";

export function parseEndpoint(value: string): URL {
  let endpoint: URL;
  try {
    endpoint = new URL(value);
  } catch (error) {
    throw new AdapterProblem(
      "invalid_endpoint",
      "MCP endpoint must be an absolute URL.",
      {
        code: "invalid_url",
        cause: error,
      },
    );
  }

  if (endpoint.username.length > 0 || endpoint.password.length > 0) {
    throw new AdapterProblem(
      "invalid_endpoint",
      "MCP endpoint must not contain user information.",
      {
        code: "userinfo_forbidden",
      },
    );
  }
  if (endpoint.hash.length > 0) {
    throw new AdapterProblem(
      "invalid_endpoint",
      "MCP endpoint must not contain a fragment.",
      {
        code: "fragment_forbidden",
      },
    );
  }
  if (endpoint.protocol === "https:") {
    return endpoint;
  }
  if (endpoint.protocol === "http:" && isLoopbackHost(endpoint.hostname)) {
    return endpoint;
  }
  throw new AdapterProblem(
    "invalid_endpoint",
    "MCP endpoint must use HTTPS; HTTP is allowed only for an explicit loopback host.",
    { code: "insecure_endpoint" },
  );
}

function isLoopbackHost(hostname: string): boolean {
  const host = hostname.toLowerCase();
  if (host === "localhost" || host === "[::1]") {
    return true;
  }
  const octets = host.split(".");
  if (octets.length !== 4 || octets[0] !== "127") {
    return false;
  }
  return octets.every(
    (octet) => /^(?:0|[1-9][0-9]{0,2})$/.test(octet) && Number(octet) <= 255,
  );
}
