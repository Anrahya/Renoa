import { dirname, join } from "node:path";

import type { CatalogEntry } from "./catalog.js";
import { validateModelSpec } from "./catalog.js";
import {
  isValidOpenCodeModelId,
  MAX_CATALOG_MODELS,
  readOpenCodeCatalogCache,
  writeOpenCodeCatalogCache,
} from "./opencode-catalog-cache.js";
import {
  OPENCODE_GO_BASE_URL,
  opencodeGoTransport,
  type OpenCodeTransport,
} from "./providers/opencode-go.js";
import { getSupportedThinkingLevels } from "./upstream/thinking.js";
import type { Api, Model, ModelCost, ThinkingLevelMap } from "./upstream/types.js";

const OFFICIAL_MODELS_URL = "https://opencode.ai/zen/go/v1/models";
const MODELS_DEV_URL = "https://models.dev/api.json";
const REQUEST_TIMEOUT_MS = 5_000;
const MAX_OFFICIAL_BYTES = 256 * 1024;
const MAX_MODELS_DEV_BYTES = 8 * 1024 * 1024;
const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

export interface OpenCodeCatalogOptions {
  readonly cachePath?: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
  readonly warn?: (message: string) => void;
}

/**
 * Resolves OpenCode Go's current availability while keeping every learned
 * model binding stable. Bundled entries win; a previously cached dynamic
 * binding wins over later metadata changes until Renoa deliberately updates it.
 */
export async function loadOpenCodeCatalog(
  authStorePath: string,
  pinned: readonly CatalogEntry[],
  options: OpenCodeCatalogOptions = {},
): Promise<readonly CatalogEntry[]> {
  const cachePath = options.cachePath ?? join(dirname(authStorePath), "opencode-go-catalog-v1.json");
  const cached = await readOpenCodeCatalogCache(cachePath, pinned);
  const fetcher = options.fetch ?? globalThis.fetch;
  const signal = requestSignal(options.signal, options.timeoutMs ?? REQUEST_TIMEOUT_MS);
  const [availabilityResult, metadataResult] = await Promise.allSettled([
    fetchOfficialModelIds(fetcher, signal),
    fetchModelsDev(fetcher, signal, cached?.models_dev_etag).then((result) =>
      result === undefined
        ? undefined
        : {
            models: projectModelsDev(result.body, pinned),
            etag: result.etag,
          },
    ),
  ]);

  const availableIds =
    availabilityResult.status === "fulfilled"
      ? availabilityResult.value
      : cached?.available_model_ids;
  let dynamicModels = [...(cached?.dynamic_models ?? [])];
  let modelsDevEtag = cached?.models_dev_etag;
  if (metadataResult.status === "fulfilled" && metadataResult.value !== undefined) {
    const existing = new Set(dynamicModels.map((model) => model.id));
    for (const model of metadataResult.value.models) {
      if (!existing.has(model.id)) {
        existing.add(model.id);
        dynamicModels.push(model);
      }
    }
    modelsDevEtag = metadataResult.value.etag;
  }

  if (availableIds !== undefined) {
    const resolved = selectAvailableModels(pinned, dynamicModels, availableIds);
    if (resolved.length > 0) {
      if (availabilityResult.status === "fulfilled" || metadataResult.status === "fulfilled") {
        const cache = {
          ...(modelsDevEtag === undefined ? {} : { models_dev_etag: modelsDevEtag }),
          available_model_ids: availableIds,
          dynamic_models: dynamicModels,
        };
        try {
          await writeOpenCodeCatalogCache(cachePath, cache);
        } catch (error) {
          warn(options, `OpenCode Go catalog cache could not be updated: ${message(error)}`);
        }
      }
      warnFetchFailures(options, availabilityResult, metadataResult);
      return resolved;
    }
  }

  warnFetchFailures(options, availabilityResult, metadataResult);
  warn(options, "OpenCode Go live catalog is unavailable; using Renoa's bundled catalog");
  return pinned;
}

function selectAvailableModels(
  pinned: readonly CatalogEntry[],
  dynamic: readonly Model<Api>[],
  availableIds: readonly string[],
): CatalogEntry[] {
  const available = new Set(availableIds);
  const byId = new Map(pinned.map((entry) => [entry.id, entry]));
  for (const model of dynamic) {
    if (available.has(model.id) && !byId.has(model.id)) {
      byId.set(model.id, toEntry(model));
    }
  }
  return [...byId.values()]
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
}

async function fetchOfficialModelIds(
  fetcher: typeof globalThis.fetch,
  signal: AbortSignal,
): Promise<string[]> {
  const response = await fetcher(OFFICIAL_MODELS_URL, {
    headers: { accept: "application/json" },
    redirect: "error",
    signal,
  });
  if (response.status !== 200) {
    throw new Error(`official model endpoint returned HTTP ${response.status}`);
  }
  const body = await readBoundedJson(response, MAX_OFFICIAL_BYTES);
  if (
    !isRecord(body) ||
    !Array.isArray(body.data) ||
    body.data.length === 0 ||
    body.data.length > MAX_CATALOG_MODELS
  ) {
    throw new Error("official model endpoint returned an invalid model list");
  }
  const ids: string[] = [];
  const unique = new Set<string>();
  for (const value of body.data) {
    const id = isRecord(value) ? value.id : undefined;
    if (!isValidOpenCodeModelId(id) || !unique.add(id)) {
      throw new Error("official model endpoint returned an invalid model id");
    }
    ids.push(id);
  }
  return ids.sort();
}

async function fetchModelsDev(
  fetcher: typeof globalThis.fetch,
  signal: AbortSignal,
  etag: string | undefined,
): Promise<{ readonly body: unknown; readonly etag?: string } | undefined> {
  const response = await fetcher(MODELS_DEV_URL, {
    headers: {
      accept: "application/json",
      ...(etag === undefined ? {} : { "if-none-match": etag }),
    },
    redirect: "error",
    signal,
  });
  if (response.status === 304) {
    if (etag === undefined) {
      throw new Error("models.dev returned 304 without a cached catalog");
    }
    return undefined;
  }
  if (response.status !== 200) {
    throw new Error(`models.dev returned HTTP ${response.status}`);
  }
  const responseEtag = response.headers.get("etag") ?? undefined;
  if (responseEtag !== undefined && responseEtag.length > 1_024) {
    throw new Error("models.dev returned an oversized ETag");
  }
  return {
    body: await readBoundedJson(response, MAX_MODELS_DEV_BYTES),
    ...(responseEtag === undefined ? {} : { etag: responseEtag }),
  };
}

function projectModelsDev(value: unknown, pinned: readonly CatalogEntry[]): Model<Api>[] {
  const provider = isRecord(value) ? value["opencode-go"] : undefined;
  const models = isRecord(provider) ? provider.models : undefined;
  const providerNpm = isRecord(provider) && typeof provider.npm === "string" ? provider.npm : undefined;
  if (
    !isRecord(models) ||
    Object.keys(models).length === 0 ||
    Object.keys(models).length > MAX_CATALOG_MODELS
  ) {
    throw new Error("models.dev returned an invalid OpenCode Go catalog");
  }
  const pinnedIds = new Set(pinned.map((entry) => entry.id));
  const projected: Model<Api>[] = [];
  for (const [id, value] of Object.entries(models).sort(([left], [right]) => left.localeCompare(right))) {
    if (!isValidOpenCodeModelId(id) || !isRecord(value) || value.id !== id) {
      throw new Error("models.dev returned an invalid OpenCode Go model");
    }
    if (pinnedIds.has(id)) {
      continue;
    }
    const model = projectModel(id, value, providerNpm);
    if (model !== undefined) {
      projected.push(validateModelSpec(model, "opencode-go", id));
    }
  }
  return projected;
}

function projectModel(
  id: string,
  value: Record<string, unknown>,
  providerNpm: string | undefined,
): Record<string, unknown> | undefined {
  const name = value.name;
  const modelProvider = isRecord(value.provider) ? value.provider : undefined;
  const npm = typeof modelProvider?.npm === "string" ? modelProvider.npm : providerNpm;
  const transport = opencodeGoTransport(id, npm);
  const modalities = isRecord(value.modalities) ? value.modalities : undefined;
  const input = Array.isArray(modalities?.input) ? modalities.input : undefined;
  const output = Array.isArray(modalities?.output) ? modalities.output : undefined;
  const limit = isRecord(value.limit) ? value.limit : undefined;
  if (
    typeof name !== "string" ||
    name.length === 0 ||
    name.length > 200 ||
    transport === undefined ||
    value.tool_call !== true ||
    !input?.includes("text") ||
    !output?.includes("text") ||
    !isPositiveSafeInteger(limit?.context) ||
    !isPositiveSafeInteger(limit?.output)
  ) {
    return undefined;
  }
  const reasoning = value.reasoning === true;
  const thinkingLevelMap = reasoning ? projectThinkingLevels(value.reasoning_options) : undefined;
  return {
    id,
    name,
    api: transport,
    provider: "opencode-go",
    baseUrl: OPENCODE_GO_BASE_URL[transport],
    reasoning,
    input: input.includes("image") ? ["text", "image"] : ["text"],
    cost: projectCost(value.cost),
    contextWindow: limit.context,
    maxTokens: limit.output,
    ...compatibility(transport, value),
    ...(thinkingLevelMap === undefined ? {} : { thinkingLevelMap }),
  };
}

function compatibility(
  transport: OpenCodeTransport,
  value: Record<string, unknown>,
): Record<string, unknown> {
  if (transport === "anthropic-messages") {
    return {};
  }
  if (transport === "openai-responses") {
    return { compat: { sessionAffinityFormat: "openai-nosession" } };
  }
  const interleaved = isRecord(value.interleaved) ? value.interleaved : undefined;
  return {
    compat: {
      supportsStore: false,
      supportsDeveloperRole: false,
      maxTokensField: "max_tokens",
      supportsStrictMode: value.structured_output === true,
      ...(interleaved?.field === "reasoning_content"
        ? { requiresReasoningContentOnAssistantMessages: true }
        : {}),
    },
  };
}

function projectThinkingLevels(value: unknown): ThinkingLevelMap | undefined {
  if (!Array.isArray(value)) {
    return undefined;
  }
  const effort = value.find((entry) => isRecord(entry) && entry.type === "effort");
  if (!isRecord(effort) || !Array.isArray(effort.values)) {
    return undefined;
  }
  const map: ThinkingLevelMap = Object.fromEntries(THINKING_LEVELS.map((level) => [level, null]));
  let supported = 0;
  for (const value of effort.values) {
    const level = value === "none" ? "off" : value;
    if (typeof level === "string" && THINKING_LEVELS.includes(level as (typeof THINKING_LEVELS)[number])) {
      map[level as keyof ThinkingLevelMap] = value as string;
      supported += 1;
    }
  }
  return supported === 0 ? undefined : map;
}

function projectCost(value: unknown): ModelCost {
  if (!isRecord(value)) {
    throw new Error("models.dev returned invalid OpenCode Go pricing");
  }
  const cost: ModelCost = {
    input: nonNegative(value.input),
    output: nonNegative(value.output),
    cacheRead: nonNegative(value.cache_read ?? 0),
    cacheWrite: nonNegative(value.cache_write ?? 0),
  };
  if (Array.isArray(value.tiers)) {
    if (value.tiers.length > 8) {
      throw new Error("models.dev returned too many OpenCode Go pricing tiers");
    }
    cost.tiers = value.tiers.map((entry) => {
      const tier = isRecord(entry) ? entry.tier : undefined;
      if (!isRecord(entry) || !isRecord(tier) || tier.type !== "context" || !isPositiveSafeInteger(tier.size)) {
        throw new Error("models.dev returned an invalid OpenCode Go pricing tier");
      }
      return {
        inputTokensAbove: tier.size,
        input: nonNegative(entry.input),
        output: nonNegative(entry.output),
        cacheRead: nonNegative(entry.cache_read ?? 0),
        cacheWrite: nonNegative(entry.cache_write ?? 0),
      };
    });
  }
  return cost;
}

async function readBoundedJson(response: Response, maximumBytes: number): Promise<unknown> {
  const length = response.headers.get("content-length");
  if (length !== null && (!/^\d+$/u.test(length) || Number(length) > maximumBytes)) {
    throw new Error("catalog response exceeds its byte limit");
  }
  if (response.body === null) {
    throw new Error("catalog response has no body");
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    bytes += value.byteLength;
    if (bytes > maximumBytes) {
      await reader.cancel();
      throw new Error("catalog response exceeds its byte limit");
    }
    chunks.push(value);
  }
  const buffer = Buffer.concat(chunks.map((chunk) => Buffer.from(chunk)), bytes);
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)) as unknown;
}

function requestSignal(parent: AbortSignal | undefined, timeoutMs: number): AbortSignal {
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error("OpenCode Go catalog timeout must be a positive integer");
  }
  const timeout = AbortSignal.timeout(timeoutMs);
  return parent === undefined ? timeout : AbortSignal.any([parent, timeout]);
}

function toEntry(model: Model<Api>): CatalogEntry {
  return {
    id: model.id,
    name: model.name,
    reasoning_levels: getSupportedThinkingLevels(model),
    model,
  };
}

function warnFetchFailures(
  options: OpenCodeCatalogOptions,
  availability: PromiseSettledResult<string[]>,
  metadata: PromiseSettledResult<unknown>,
): void {
  if (availability.status === "rejected") {
    warn(options, `OpenCode Go availability refresh failed: ${message(availability.reason)}`);
  }
  if (metadata.status === "rejected") {
    warn(options, `OpenCode Go metadata refresh failed: ${message(metadata.reason)}`);
  }
}

function warn(options: OpenCodeCatalogOptions, value: string): void {
  (options.warn ?? console.error)(value);
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function nonNegative(value: unknown): number {
  if (typeof value !== "number" || !Number.isFinite(value) || value < 0) {
    throw new Error("models.dev returned invalid OpenCode Go pricing");
  }
  return value;
}

function isPositiveSafeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
