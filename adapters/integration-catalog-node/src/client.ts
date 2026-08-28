import { createHash } from "node:crypto";

import type {
  AdapterRecord,
  CatalogAuth,
  CatalogCandidate,
  CatalogRequest,
} from "./contract.js";
import { CatalogProblem } from "./errors.js";
import {
  ADAPTER_REVISION,
  CATALOG_SCHEMA_VERSION,
  MAX_RESPONSE_BYTES,
  MAX_SEARCH_RESULTS,
  REQUEST_TIMEOUT_MS,
  WIRE_VERSION,
} from "./limits.js";
import { parseReference } from "./wire.js";

const DEFAULT_BASE_URL = "https://integrations.sh";

export async function execute(
  request: CatalogRequest,
  options: { readonly baseUrl?: string; readonly signal?: AbortSignal } = {},
): Promise<AdapterRecord> {
  const baseUrl = validateBaseUrl(options.baseUrl ?? DEFAULT_BASE_URL);
  const operationSignal =
    options.signal === undefined
      ? AbortSignal.timeout(REQUEST_TIMEOUT_MS)
      : AbortSignal.any([
          options.signal,
          AbortSignal.timeout(REQUEST_TIMEOUT_MS),
        ]);
  if (request.action === "search") {
    const results = await findSearchResults(
      baseUrl,
      request.query,
      operationSignal,
    );
    const candidates: CatalogCandidate[] = [];
    for (const result of results.slice(0, 5)) {
      const surface = await getJson(
        baseUrl,
        `/api/${encodeURIComponent(result.domain)}/surface`,
        operationSignal,
      );
      candidates.push(...normalizeRecord(surface));
    }
    const unique = new Map<string, CatalogCandidate>();
    for (const candidate of candidates) {
      unique.set(candidate.reference, candidate);
    }
    return {
      wire_version: WIRE_VERSION,
      event: "completed",
      adapter_revision: ADAPTER_REVISION,
      result: {
        action: "search",
        candidates: [...unique.values()].slice(0, MAX_SEARCH_RESULTS),
      },
    };
  }

  const reference = parseReference(request.candidate);
  const record = await getJson(
    baseUrl,
    `/api/${encodeURIComponent(reference.domain)}/surface`,
    operationSignal,
  );
  const candidates = normalizeRecord(record);
  const candidate = candidates.find(
    (item) => item.reference === request.candidate,
  );
  if (candidate !== undefined) {
    return {
      wire_version: WIRE_VERSION,
      event: "completed",
      adapter_revision: ADAPTER_REVISION,
      result: { action: "resolve", candidate },
    };
  }
  if (!candidates.some((item) => item.server === reference.slug)) {
    throw new CatalogProblem(
      "not_found",
      `integrations.sh no longer advertises MCP server '${reference.slug}' for '${reference.domain}'. Search again or research the MCP manually.`,
      { code: "candidate_missing" },
    );
  }
  throw new CatalogProblem(
    "conflict",
    "The integrations.sh candidate changed after discovery. Search again and review the current endpoint and authentication before adding it.",
    { code: "stale_candidate" },
  );
}

async function findSearchResults(
  baseUrl: URL,
  query: string,
  signal: AbortSignal,
): Promise<readonly { readonly domain: string }[]> {
  const direct = searchResults(
    await getJson(baseUrl, searchPath(query), signal),
  );
  if (direct.length > 0) {
    return rankResults(direct, query);
  }
  for (const token of fallbackTokens(query)) {
    const fallback = searchResults(
      await getJson(baseUrl, searchPath(token), signal),
    );
    if (fallback.length > 0) {
      return rankResults(fallback, token);
    }
  }
  return [];
}

function searchPath(query: string): string {
  return `/api/search?q=${encodeURIComponent(query)}&kind=mcp&limit=${MAX_SEARCH_RESULTS}`;
}

function fallbackTokens(query: string): string[] {
  const ignored = new Set([
    "a",
    "add",
    "an",
    "extension",
    "for",
    "install",
    "integration",
    "mcp",
    "me",
    "plugin",
    "the",
    "to",
    "with",
  ]);
  const observed = new Set<string>();
  return query
    .toLowerCase()
    .split(/[^a-z0-9.-]+/u)
    .filter((token) => token.length >= 2 && !ignored.has(token))
    .filter((token) => {
      if (observed.has(token)) {
        return false;
      }
      observed.add(token);
      return true;
    })
    .slice(0, 4);
}

function rankResults(
  results: readonly { readonly domain: string }[],
  query: string,
): { readonly domain: string }[] {
  const needle = query.toLowerCase();
  const ranked = [...results].sort((left, right) => {
    const leftHost = left.domain.split(".")[0] ?? left.domain;
    const rightHost = right.domain.split(".")[0] ?? right.domain;
    const leftRank = leftHost === needle ? 0 : leftHost.startsWith(needle) ? 1 : 2;
    const rightRank = rightHost === needle ? 0 : rightHost.startsWith(needle) ? 1 : 2;
    return leftRank - rightRank;
  });
  const exact = ranked.filter(
    (result) => (result.domain.split(".")[0] ?? result.domain) === needle,
  );
  return exact.length > 0 ? exact : ranked;
}

function searchResults(value: unknown): readonly { readonly domain: string }[] {
  const root = requireObject(value, "search response");
  if (!Array.isArray(root.results)) {
    throw protocol("integrations.sh search response has no results array");
  }
  const output: { domain: string }[] = [];
  for (const value of root.results.slice(0, MAX_SEARCH_RESULTS)) {
    const result = requireObject(value, "search result");
    const domain = requireDomain(result.domain, "search result domain");
    const kinds = result.kinds;
    if (Array.isArray(kinds) && kinds.includes("mcp")) {
      output.push({ domain });
    }
  }
  return output;
}

function normalizeRecord(value: unknown): CatalogCandidate[] {
  const root = requireObject(value, "surface response");
  if (root.version !== CATALOG_SCHEMA_VERSION) {
    throw protocol(
      `integrations.sh surface schema is ${String(root.version)}, expected ${CATALOG_SCHEMA_VERSION}`,
    );
  }
  const domain = requireDomain(root.domain, "surface domain");
  const description = optionalString(root.description) ?? "";
  if (!Array.isArray(root.surfaces)) {
    throw protocol("integrations.sh surface response has no surfaces array");
  }
  const candidates: CatalogCandidate[] = [];
  for (const value of root.surfaces) {
    const surface = requireObject(value, "surface");
    if (surface.type !== "mcp") {
      continue;
    }
    const transports = surface.transports;
    if (!Array.isArray(transports) || !transports.includes("streamable-http")) {
      continue;
    }
    const endpoint = requireHttpsUrl(surface.url, "MCP endpoint");
    const server = requireSlug(surface.slug, "MCP slug");
    const name = requireNonEmptyString(surface.name, "MCP name");
    const docs = optionalHttpsUrl(surface.docs, "MCP docs");
    const auth = normalizeAuth(surface.auth, root.credentials);
    const basis = requireObject(surface.basis, "MCP basis");
    const evidence = evidenceUrls(basis.evidence);
    const record = `https://integrations.sh/${domain}/`;
    const unsigned = {
      name,
      description,
      domain,
      server,
      endpoint,
      transport: "streamable-http" as const,
      ...(docs === undefined ? {} : { docs }),
      auth,
      source: {
        provider: "integrations.sh" as const,
        record,
        evidence,
      },
    };
    const digest = createHash("sha256")
      .update(canonicalJson(unsigned))
      .digest("hex");
    candidates.push({
      reference: `integrations.sh/${domain}/${server}/${digest}`,
      ...unsigned,
    });
  }
  return candidates;
}

function normalizeAuth(value: unknown, credentials: unknown): CatalogAuth {
  const auth = requireObject(value, "MCP auth");
  if (auth.status === "none") {
    return { status: "none" };
  }
  const status =
    auth.status === "required" || auth.status === "optional"
      ? auth.status
      : "unknown";
  const setup = credentialSetup(credentials);
  return {
    status,
    ...(setup === undefined ? {} : { setup }),
    blocker:
      "This MCP requires credential setup that Renoa cannot provision through catalog discovery yet. Do not request a secret in chat; explain the setup requirement or use an already stored Secret Service bearer credential through the expert connect action.",
  };
}

function credentialSetup(value: unknown): string | undefined {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return undefined;
  }
  for (const credential of Object.values(value)) {
    if (typeof credential === "object" && credential !== null && !Array.isArray(credential)) {
      const setup = optionalString((credential as Record<string, unknown>).setup);
      if (setup !== undefined) {
        return setup.slice(0, 8_192);
      }
    }
  }
  return undefined;
}

async function getJson(
  baseUrl: URL,
  path: string,
  parentSignal: AbortSignal | undefined,
): Promise<unknown> {
  const timeout = AbortSignal.timeout(REQUEST_TIMEOUT_MS);
  const signal =
    parentSignal === undefined
      ? timeout
      : AbortSignal.any([parentSignal, timeout]);
  let response: Response;
  try {
    response = await fetch(new URL(path, baseUrl), {
      method: "GET",
      headers: { accept: "application/json" },
      redirect: "error",
      signal,
    });
  } catch (error) {
    throw new CatalogProblem(
      "unavailable",
      "integrations.sh discovery is unavailable. Use web research or a local Agent Plugin package instead; do not guess an MCP endpoint.",
      { code: "catalog_unavailable", cause: error },
    );
  }
  if (!response.ok) {
    throw new CatalogProblem(
      response.status === 404 ? "not_found" : "unavailable",
      `integrations.sh returned HTTP ${response.status}. Use web research or a local Agent Plugin package if discovery remains unavailable.`,
      {
        code: "catalog_http_error",
        httpStatus: response.status,
      },
    );
  }
  const bytes = await readBounded(response);
  try {
    return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(bytes)) as unknown;
  } catch (error) {
    throw new CatalogProblem(
      "protocol",
      "integrations.sh returned malformed JSON. Use web research or a local Agent Plugin package instead.",
      { code: "invalid_catalog_json", cause: error },
    );
  }
}

async function readBounded(response: Response): Promise<Uint8Array> {
  if (response.body === null) {
    throw protocol("integrations.sh returned an empty response body");
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  while (true) {
    const item = await reader.read();
    if (item.done) {
      break;
    }
    bytes += item.value.byteLength;
    if (bytes > MAX_RESPONSE_BYTES) {
      await reader.cancel();
      throw new CatalogProblem(
        "resource_limit",
        `integrations.sh response exceeds ${MAX_RESPONSE_BYTES} bytes.`,
        { code: "catalog_response_limit" },
      );
    }
    chunks.push(item.value);
  }
  const output = new Uint8Array(bytes);
  let offset = 0;
  for (const chunk of chunks) {
    output.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return output;
}

function validateBaseUrl(value: string): URL {
  const url = new URL(value);
  const loopback = url.hostname === "127.0.0.1" || url.hostname === "[::1]" || url.hostname === "localhost";
  if (
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.search.length > 0 ||
    url.hash.length > 0 ||
    (url.protocol !== "https:" && !(url.protocol === "http:" && loopback))
  ) {
    throw new CatalogProblem(
      "invalid_request",
      "integration catalog base URL must be HTTPS, or HTTP loopback for tests",
      { code: "invalid_catalog_base_url" },
    );
  }
  return url;
}

function evidenceUrls(value: unknown): string[] {
  if (!Array.isArray(value)) {
    return [];
  }
  const unique = new Set<string>();
  for (const item of value) {
    const url = optionalHttpsUrl(item, "MCP evidence");
    if (url !== undefined) {
      unique.add(url);
    }
  }
  return [...unique].sort().slice(0, 32);
}

function canonicalJson(value: unknown): string {
  if (Array.isArray(value)) {
    return `[${value.map(canonicalJson).join(",")}]`;
  }
  if (typeof value === "object" && value !== null) {
    return `{${Object.entries(value)
      .sort(([left], [right]) => left.localeCompare(right))
      .map(([key, item]) => `${JSON.stringify(key)}:${canonicalJson(item)}`)
      .join(",")}}`;
  }
  return JSON.stringify(value);
}

function requireObject(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw protocol(`${path} must be an object`);
  }
  return value as Record<string, unknown>;
}

function requireNonEmptyString(value: unknown, path: string): string {
  if (typeof value !== "string" || value.length === 0) {
    throw protocol(`${path} must be a non-empty string`);
  }
  return value;
}

function optionalString(value: unknown): string | undefined {
  return typeof value === "string" && value.length > 0 ? value : undefined;
}

function requireDomain(value: unknown, path: string): string {
  const domain = requireNonEmptyString(value, path).toLowerCase();
  if (!/^[a-z0-9.-]+$/u.test(domain) || !domain.includes(".")) {
    throw protocol(`${path} is malformed`);
  }
  return domain;
}

function requireSlug(value: unknown, path: string): string {
  const slug = requireNonEmptyString(value, path);
  if (!/^[a-z0-9-]+$/u.test(slug)) {
    throw protocol(`${path} is malformed`);
  }
  return slug;
}

function requireHttpsUrl(value: unknown, path: string): string {
  const url = optionalHttpsUrl(value, path);
  if (url === undefined) {
    throw protocol(`${path} must be an HTTPS URL`);
  }
  return url;
}

function optionalHttpsUrl(value: unknown, path: string): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "string") {
    throw protocol(`${path} must be a string`);
  }
  let url: URL;
  try {
    url = new URL(value);
  } catch (error) {
    throw new CatalogProblem("protocol", `${path} is invalid`, {
      code: "invalid_catalog_record",
      cause: error,
    });
  }
  if (
    url.protocol !== "https:" ||
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.hash.length > 0
  ) {
    throw protocol(`${path} must be an HTTPS URL without credentials or fragment`);
  }
  return url.toString();
}

function protocol(message: string): CatalogProblem {
  return new CatalogProblem("protocol", message, {
    code: "invalid_catalog_record",
  });
}
