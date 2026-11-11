import { createHash } from "node:crypto";

import { getSupportedThinkingLevels, type Api, type Model } from "@earendil-works/pi-ai";
import { ANTHROPIC_MODELS } from "@earendil-works/pi-ai/providers/anthropic.models";
import { DEEPSEEK_MODELS } from "@earendil-works/pi-ai/providers/deepseek.models";
import { OPENCODE_GO_MODELS } from "@earendil-works/pi-ai/providers/opencode-go.models";
import { XAI_MODELS } from "@earendil-works/pi-ai/providers/xai.models";

import type { CatalogModel, ProviderId, ReasoningLevel } from "./contract.js";
import {
  OPENCODE_GO_BASE_URL,
  opencodeGoTransport,
  type OpenCodeTransport,
} from "./providers/opencode-go.js";
import { ANTHROPIC_BASE_URL } from "./providers/anthropic.js";
import { DEEPSEEK_BASE_URL } from "./providers/deepseek.js";
import { XAI_BASE_URL } from "./providers/xai.js";

export interface CatalogEntry {
  readonly id: string;
  readonly name: string;
  readonly reasoning_levels: readonly ReasoningLevel[];
  readonly model: Model<Api>;
}

/**
 * Catalog metadata that classifies or annotates a Pi entry but that no retained
 * transport consumes. It is removed before an entry becomes a model binding.
 */
const UNCONSUMED_CATALOG_FIELDS = ["type", "inputLimits"] as const;

/**
 * Pi compat switches whose behaviour Renoa's contract cannot carry: one system
 * prompt, one tool list and one effort per request, and no reply from a model
 * other than the one bound. Mid-conversation effort markers would also be
 * missing from replayed history, which carries no per-turn effort, so every
 * replayed thinking block would fail its prefix check.
 */
const UNCARRIED_COMPAT_FIELDS = [
  "supportsMidConvoEffort",
  "supportsMidConvoSystemMessages",
  "supportsMidConvoToolAdditions",
  "supportsMidConvoToolChanges",
  "supportsAdditionalTools",
  "allowedFallbackModels",
] as const;

/** Pi's chat catalogs, keyed by model id, as the pinned package publishes them. */
const PINNED_CATALOGS: Readonly<Record<ProviderId, Readonly<Record<string, object>>>> = {
  anthropic: ANTHROPIC_MODELS,
  xai: XAI_MODELS,
  "opencode-go": OPENCODE_GO_MODELS,
  deepseek: DEEPSEEK_MODELS,
};

export function loadPinnedCatalog(provider: ProviderId): readonly CatalogEntry[] {
  const discovered = Object.values(PINNED_CATALOGS[provider])
    .map((entry) => ({ ...entry }) as Record<string, unknown>)
    .filter((entry) => isAdvertisableCatalogEntry(provider, entry))
    .map(withoutUnconsumedFields)
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

/**
 * Pi's chat catalogs also list models on APIs this adapter does not carry, such
 * as Google Generative AI, and discovered OpenCode Go entries may be image or
 * classifier models. Only chat models on a retained API are advertised; an
 * entry without `type` is chat.
 */
export function isAdvertisableCatalogEntry(
  provider: ProviderId,
  entry: Record<string, unknown>,
): boolean {
  return (entry.type === undefined || entry.type === "chat") && supportsApi(provider, entry.api);
}

function withoutUnconsumedFields(entry: Record<string, unknown>): Record<string, unknown> {
  const next = { ...entry };
  for (const field of UNCONSUMED_CATALOG_FIELDS) {
    delete next[field];
  }
  if (isRecord(next.compat)) {
    const compat = { ...next.compat };
    for (const field of UNCARRIED_COMPAT_FIELDS) {
      delete compat[field];
    }
    next.compat = compat;
  }
  return next;
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
  // A compat block describes the API it was written for: it is kept when
  // only the endpoint moves, and dropped when the entry changes API.
  if (official === "anthropic-messages" && entry.api !== official) {
    delete next.compat;
  }
  return next;
}

function supportsApi(provider: ProviderId, api: unknown): api is Api {
  switch (provider) {
    case "anthropic":
      return api === "anthropic-messages";
    case "xai":
      return api === "openai-responses";
    case "deepseek":
      return api === "openai-completions";
    case "opencode-go":
      return api === "anthropic-messages" || api === "openai-completions" || api === "openai-responses";
  }
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
  if (provider === "anthropic") {
    return value === ANTHROPIC_BASE_URL;
  }
  if (provider === "xai") {
    return value === XAI_BASE_URL;
  }
  if (provider === "deepseek") {
    return value === DEEPSEEK_BASE_URL;
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
