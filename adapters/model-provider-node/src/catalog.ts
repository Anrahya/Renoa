import { createHash } from "node:crypto";

import xaiCatalog from "./upstream/catalogs/xai.json" with { type: "json" };
import opencodeGoCatalog from "./upstream/catalogs/opencode-go.json" with { type: "json" };
import type { CatalogModel, ProviderId, ReasoningLevel } from "./contract.js";
import {
  OPENCODE_GO_BASE_URL,
  OPENCODE_GO_CATALOG_ADDITIONS,
  opencodeGoTransport,
  type OpenCodeTransport,
} from "./providers/opencode-go.js";
import { XAI_BASE_URL } from "./providers/xai.js";
import { getSupportedThinkingLevels } from "./upstream/thinking.js";
import type { Api, Model } from "./upstream/types.js";

export interface CatalogEntry {
  readonly id: string;
  readonly name: string;
  readonly reasoning_levels: readonly ReasoningLevel[];
  readonly model: Model<Api>;
}

export function loadPinnedCatalog(provider: ProviderId): readonly CatalogEntry[] {
  const raw = provider === "xai" ? xaiCatalog : opencodeGoCatalog;
  const pinned = flattenCatalog(raw);
  const pinnedIds = new Set(pinned.map((entry) => entry.id));
  const source =
    provider === "opencode-go"
      ? [
          ...pinned,
          ...OPENCODE_GO_CATALOG_ADDITIONS.filter((entry) => !pinnedIds.has(entry.id)),
        ]
      : pinned;
  const discovered = source
    .map((entry) => (provider === "opencode-go" ? applyOpenCodeOfficialTransport(entry) : entry))
    .filter((entry): entry is Record<string, unknown> => entry !== undefined)
    .map((entry) => validateModelSpec(entry, provider, String(entry.id)));
  return discovered
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
    .map((model) => ({
      id: model.id,
      name: model.name,
      reasoning_levels: getSupportedThinkingLevels(model),
      model,
    }));
}

export function toWireCatalog(entries: readonly CatalogEntry[]): readonly CatalogModel[] {
  return entries.map((entry) => ({
    id: entry.id,
    name: entry.name,
    reasoning_levels: entry.reasoning_levels,
    context_window_tokens: entry.model.contextWindow,
    model_spec: entry.model,
  }));
}

export function findCatalogModel(
  provider: ProviderId,
  modelId: string,
): CatalogEntry | undefined {
  return loadPinnedCatalog(provider).find((entry) => entry.id === modelId);
}

export function modelBindingId(model: Model<Api>): string {
  return createHash("sha256").update(JSON.stringify(model)).digest("hex");
}

export function resolveReasoningLevel(
  model: Model<Api>,
  requested: ReasoningLevel | undefined,
): ReasoningLevel {
  const supported = getSupportedThinkingLevels(model);
  if (requested !== undefined) {
    if (!supported.includes(requested)) {
      throw new Error(`${model.id} does not support ${requested} reasoning`);
    }
    return requested;
  }
  return supported.includes("high") ? "high" : (supported[0] ?? "off");
}

export function validateModelSpec(
  value: unknown,
  provider: ProviderId,
  modelId: string,
  options: { readonly allowLoopback?: boolean } = {},
): Model<Api> {
  if (!isRecord(value)) {
    throw new Error(`model binding for ${provider}/${modelId} is invalid`);
  }
  const cost = value.cost;
  const loopback = options.allowLoopback === true;
  if (
    value.id !== modelId ||
    typeof value.name !== "string" ||
    value.name.length === 0 ||
    !supportsApi(provider, value.api) ||
    value.provider !== provider ||
    !trustedBaseUrl(provider, value.api, value.baseUrl, loopback) ||
    (provider === "xai" && value.headers !== undefined) ||
    (value.compat !== undefined && !isRecord(value.compat)) ||
    (value.samplingParams !== undefined && !isRecord(value.samplingParams)) ||
    (value.thinkingLevelMap !== undefined && !isRecord(value.thinkingLevelMap)) ||
    typeof value.reasoning !== "boolean" ||
    !Array.isArray(value.input) ||
    !value.input.includes("text") ||
    !value.input.every((entry) => entry === "text" || entry === "image") ||
    !isRecord(cost) ||
    !["input", "output", "cacheRead", "cacheWrite"].every((field) =>
      isNonNegativeNumber(cost[field]),
    ) ||
    !isPositiveSafeInteger(value.contextWindow) ||
    !isPositiveSafeInteger(value.maxTokens)
  ) {
    throw new Error(`model binding for ${provider}/${modelId} is invalid`);
  }
  return structuredClone(value) as unknown as Model<Api>;
}

function applyOpenCodeOfficialTransport(
  entry: Record<string, unknown>,
): Record<string, unknown> | undefined {
  const modelId = typeof entry.id === "string" ? entry.id : undefined;
  if (modelId === undefined) {
    return undefined;
  }
  const official = opencodeGoTransport(modelId);
  if (official === undefined) {
    return undefined;
  }
  if (entry.api === official && entry.baseUrl === OPENCODE_GO_BASE_URL[official]) {
    return entry;
  }
  const next: Record<string, unknown> = {
    ...entry,
    api: official,
    baseUrl: OPENCODE_GO_BASE_URL[official],
  };
  if (official === "anthropic-messages") {
    delete next.compat;
  }
  return next;
}

function flattenCatalog(value: unknown): Record<string, unknown>[] {
  if (!isRecord(value)) {
    return [];
  }
  const entries: Record<string, unknown>[] = [];
  for (const group of Object.values(value)) {
    if (!isRecord(group)) {
      continue;
    }
    for (const model of Object.values(group)) {
      if (isRecord(model)) {
        entries.push(model);
      }
    }
  }
  return entries;
}

function supportsApi(provider: ProviderId, api: unknown): api is Api {
  return provider === "xai"
    ? api === "openai-completions" || api === "openai-responses"
    : api === "anthropic-messages" || api === "openai-completions" || api === "openai-responses";
}

function trustedBaseUrl(
  provider: ProviderId,
  api: unknown,
  value: unknown,
  allowLoopback: boolean,
): boolean {
  if (typeof value !== "string") {
    return false;
  }
  if (allowLoopback && isLoopbackBaseUrl(value)) {
    return true;
  }
  if (provider === "xai") {
    return value === XAI_BASE_URL;
  }
  return typeof api === "string" && api in OPENCODE_GO_BASE_URL
    ? value === OPENCODE_GO_BASE_URL[api as OpenCodeTransport]
    : false;
}

function isLoopbackBaseUrl(value: string): boolean {
  let url: URL;
  try {
    url = new URL(value);
  } catch {
    return false;
  }
  return (
    (url.protocol === "http:" || url.protocol === "https:") &&
    (url.hostname === "127.0.0.1" || url.hostname === "localhost")
  );
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isNonNegativeNumber(value: unknown): value is number {
  return typeof value === "number" && Number.isFinite(value) && value >= 0;
}

function isPositiveSafeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}
