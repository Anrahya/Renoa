import type {
  AdapterRecord,
  RegistryCandidate,
  RegistryRequest,
  RegistrySearchResult,
} from "./contract.js";
import { RegistryProblem } from "./errors.js";
import { getJson, registryBaseUrl, type RequestContext } from "./http.js";
import {
  ADAPTER_REVISION,
  MAX_PAGE_RESULTS,
  MAX_PAGES_PER_QUERY,
  MAX_MODEL_RESULT_BYTES,
  MAX_QUERY_TOKENS,
  MAX_QUERY_VARIANTS,
  MAX_SEARCH_RESULTS,
  OFFICIAL_REGISTRY_BASE_URL,
  REQUEST_TIMEOUT_MS,
  WIRE_VERSION,
} from "./limits.js";
import {
  REGISTRY_TRUST,
  normalizeCandidate,
  normalizeLookup,
} from "./records.js";

const SEARCH_STOP_WORDS = new Set([
  "a",
  "add",
  "an",
  "connect",
  "configure",
  "extension",
  "find",
  "for",
  "get",
  "give",
  "i",
  "install",
  "integration",
  "mcp",
  "me",
  "my",
  "need",
  "please",
  "plugin",
  "server",
  "setup",
  "search",
  "the",
  "tool",
  "tools",
  "to",
  "use",
  "using",
  "want",
  "web",
  "with",
]);

export async function execute(
  request: RegistryRequest,
  options: {
    readonly baseUrl?: string;
    readonly signal?: AbortSignal;
    readonly timeoutMs?: number;
  } = {},
): Promise<AdapterRecord> {
  const baseUrl = registryBaseUrl(
    options.baseUrl ?? OFFICIAL_REGISTRY_BASE_URL,
  );
  const timeoutSignal = AbortSignal.timeout(
    options.timeoutMs ?? REQUEST_TIMEOUT_MS,
  );
  const signal =
    options.signal === undefined
      ? timeoutSignal
      : AbortSignal.any([options.signal, timeoutSignal]);
  const context: RequestContext = {
    baseUrl,
    signal,
    ...(options.signal === undefined ? {} : { parentSignal: options.signal }),
    timeoutSignal,
  };
  const result =
    request.action === "search"
      ? await search(request.query, context)
      : await lookup(
          request.registry_name,
          request.registry_version,
          context,
        );
  return {
    wire_version: WIRE_VERSION,
    event: "completed",
    adapter_revision: ADAPTER_REVISION,
    result,
  };
}

interface QueryPlan {
  readonly tokens: readonly string[];
  readonly variants: readonly string[];
}

interface QueryPageResult {
  readonly records: readonly unknown[];
  readonly sourceTruncated: boolean;
}

async function search(
  query: string,
  context: RequestContext,
): Promise<RegistrySearchResult> {
  const plan = queryPlan(query);
  const siblingCancellation = new AbortController();
  const searchContext: RequestContext = {
    ...context,
    signal: AbortSignal.any([context.signal, siblingCancellation.signal]),
  };
  let pages: readonly QueryPageResult[];
  try {
    pages = await Promise.all(
      plan.variants.map((variant) => searchVariant(variant, searchContext)),
    );
  } catch (error) {
    siblingCancellation.abort();
    throw error;
  }

  const candidates = new Map<string, RegistryCandidate>();
  let rejectedRecords = 0;
  let filteredRecords = 0;
  for (const page of pages) {
    for (const value of page.records) {
      let normalized;
      try {
        normalized = normalizeCandidate(value, plan.tokens);
      } catch (error) {
        if (error instanceof RegistryProblem) {
          rejectedRecords += 1;
          continue;
        }
        throw error;
      }
      if (!candidateMatchesQuery(normalized.candidate, plan.tokens)) {
        filteredRecords += 1;
        continue;
      }
      const previous = candidates.get(normalized.key);
      if (
        previous !== undefined &&
        JSON.stringify(previous) !== JSON.stringify(normalized.candidate)
      ) {
        throw new RegistryProblem(
          "protocol",
          "Official MCP Registry returned conflicting metadata for one exact server version.",
          { code: "conflicting_registry_record" },
        );
      }
      candidates.set(normalized.key, normalized.candidate);
    }
  }
  const ranked = [...candidates.values()].sort((left, right) =>
    compareCandidates(left, right, plan.tokens),
  );
  const output = ranked.slice(0, MAX_SEARCH_RESULTS);
  let result = searchResult(query, plan, output, ranked.length, rejectedRecords, filteredRecords, pages);
  while (Buffer.byteLength(JSON.stringify(result), "utf8") > MAX_MODEL_RESULT_BYTES) {
    if (output.pop() === undefined) {
      throw new RegistryProblem(
        "resource_limit",
        "Official MCP Registry search metadata cannot fit Renoa's model-facing result boundary.",
        { code: "registry_result_limit" },
      );
    }
    result = searchResult(query, plan, output, ranked.length, rejectedRecords, filteredRecords, pages);
  }
  return result;
}

function searchResult(
  query: string,
  plan: QueryPlan,
  output: readonly RegistryCandidate[],
  uniqueSeen: number,
  rejectedRecords: number,
  filteredRecords: number,
  pages: readonly QueryPageResult[],
): RegistrySearchResult {
  return {
    action: "search",
    source: "official_mcp_registry",
    query,
    normalized_queries: plan.variants,
    candidates: output,
    coverage: {
      returned: output.length,
      unique_seen: uniqueSeen,
      rejected_records: rejectedRecords,
      filtered_records: filteredRecords,
      source_truncated: pages.some((page) => page.sourceTruncated),
      output_truncated: output.length < uniqueSeen,
    },
    trust: REGISTRY_TRUST,
    next_action:
      output.length === 0
        ? "Retry once with only the provider or product name. If the official Registry still has no usable name match, search the provider's official website; do not guess an endpoint."
        : "Call lookup with one exact registry_name and registry_version. Registry publication proves namespace control only; verify provider ownership, endpoint, and authentication in official provider documentation before add.",
  };
}

async function lookup(
  name: string,
  version: string,
  context: RequestContext,
) {
  const path = `/v0.1/servers/${encodeURIComponent(name)}/versions/${encodeURIComponent(version)}`;
  const value = await getJson(path, context);
  const sourceRecord = new URL(path, context.baseUrl).toString();
  const result = normalizeLookup(value, name, version, sourceRecord);
  if (Buffer.byteLength(JSON.stringify(result), "utf8") > MAX_MODEL_RESULT_BYTES) {
    throw new RegistryProblem(
      "resource_limit",
      "The exact official MCP Registry record exceeds Renoa's model-facing result boundary; no extension was installed.",
      { code: "registry_result_limit" },
    );
  }
  return result;
}

async function searchVariant(
  query: string,
  context: RequestContext,
): Promise<QueryPageResult> {
  const records: unknown[] = [];
  const cursors = new Set<string>();
  let cursor: string | undefined;
  for (let page = 0; page < MAX_PAGES_PER_QUERY; page += 1) {
    const url = new URL("/v0.1/servers", context.baseUrl);
    url.searchParams.set("search", query);
    url.searchParams.set("version", "latest");
    url.searchParams.set("limit", String(MAX_PAGE_RESULTS));
    if (cursor !== undefined) {
      url.searchParams.set("cursor", cursor);
    }
    const response = pageResponse(
      await getJson(`${url.pathname}${url.search}`, context),
    );
    records.push(...response.servers);
    if (response.nextCursor === undefined) {
      return { records, sourceTruncated: false };
    }
    if (cursors.has(response.nextCursor)) {
      throw new RegistryProblem(
        "protocol",
        "Official MCP Registry repeated a pagination cursor.",
        { code: "registry_cursor_cycle" },
      );
    }
    cursors.add(response.nextCursor);
    cursor = response.nextCursor;
  }
  return { records, sourceTruncated: cursor !== undefined };
}

function pageResponse(value: unknown): {
  readonly servers: readonly unknown[];
  readonly nextCursor?: string;
} {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw invalidPage("registry list response must be an object");
  }
  const root = value as Record<string, unknown>;
  if (!Array.isArray(root.servers) || root.servers.length > MAX_PAGE_RESULTS) {
    throw invalidPage(
      `registry list response must contain at most ${MAX_PAGE_RESULTS} servers`,
    );
  }
  const metadata = root.metadata;
  if (typeof metadata !== "object" || metadata === null || Array.isArray(metadata)) {
    throw invalidPage("registry list response metadata must be an object");
  }
  const metadataRecord = metadata as Record<string, unknown>;
  const count = metadataRecord.count;
  if (
    count !== undefined &&
    (!Number.isSafeInteger(count) || count !== root.servers.length)
  ) {
    throw invalidPage("registry list response count does not match its servers");
  }
  const nextCursor = metadataRecord.nextCursor;
  if (nextCursor === undefined || nextCursor === null || nextCursor === "") {
    return { servers: root.servers };
  }
  if (
    typeof nextCursor !== "string" ||
    Buffer.byteLength(nextCursor, "utf8") > 4_096 ||
    /[\u0000-\u001F\u007F]/u.test(nextCursor)
  ) {
    throw invalidPage("registry pagination cursor is malformed");
  }
  return { servers: root.servers, nextCursor };
}

function queryPlan(query: string): QueryPlan {
  const rawTokens = query
    .normalize("NFKC")
    .toLowerCase()
    .match(/[a-z0-9]+/gu) ?? [];
  const distinct = [...new Set(rawTokens)];
  const meaningful = distinct.filter((token) => !SEARCH_STOP_WORDS.has(token));
  const tokens = (meaningful.length > 0 ? meaningful : distinct).slice(
    0,
    MAX_QUERY_TOKENS,
  );
  const variants: string[] = [];
  const add = (value: string) => {
    if (
      value.length > 0 &&
      !variants.includes(value) &&
      variants.length < MAX_QUERY_VARIANTS
    ) {
      variants.push(value);
    }
  };
  if (tokens.length > 0) {
    add(tokens.join("-"));
    if (tokens.length > 1) {
      add(tokens.join(""));
    }
    for (const token of tokens) {
      add(token);
    }
  } else {
    add(query.trim().toLowerCase());
  }
  return { tokens, variants };
}

function compareCandidates(
  left: RegistryCandidate,
  right: RegistryCandidate,
  tokens: readonly string[],
): number {
  const rank = (candidate: RegistryCandidate): number => {
    const name = candidate.registry_name.toLowerCase();
    const leaf = name.split("/")[1] ?? name;
    const phrase = tokens.join("-");
    const haystack = `${name} ${candidate.title ?? ""}`.toLowerCase();
    let score =
      candidate.status === "active"
        ? 0
        : candidate.status === "deprecated"
          ? 100
          : 200;
    if (candidate.publisher_namespace_matches_query) {
      score += 0;
    } else if (phrase.length > 0 && leaf === phrase) {
      score += 10;
    } else if (tokens.length > 0 && tokens.every((token) => name.includes(token))) {
      score += 20;
    } else if (tokens.length > 0 && tokens.every((token) => haystack.includes(token))) {
      score += 30;
    } else {
      score += 40;
    }
    if (candidate.streamable_http_count === 0) {
      score += 5;
    }
    return score;
  };
  return (
    rank(left) - rank(right) ||
    compareText(left.registry_name, right.registry_name) ||
    compareText(left.registry_version, right.registry_version)
  );
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}

function candidateMatchesQuery(
  candidate: RegistryCandidate,
  tokens: readonly string[],
): boolean {
  if (tokens.length === 0) {
    return true;
  }
  const identity = `${candidate.registry_name} ${candidate.title ?? ""}`.toLowerCase();
  return tokens.every((token) => identity.includes(token));
}

function invalidPage(message: string): RegistryProblem {
  return new RegistryProblem("protocol", message, {
    code: "invalid_registry_response",
  });
}
