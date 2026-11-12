import { dirname, join } from "node:path";

import type { CatalogEntry } from "./catalog.js";
import { validateModelSpec } from "./catalog.js";
import {
  mergeLearnedModels,
  readBoundedJson,
  readLiveCatalogCache,
  requestSignal,
  selectAvailableModels,
  writeLiveCatalogCache,
  type LiveCatalogSource,
} from "./live-catalog.js";
import { OPENROUTER_BASE_URL } from "./providers/openrouter.js";
import type { Api, Model, ThinkingLevelMap } from "@earendil-works/pi-ai";
import { openRouterCost, type OpenRouterPricing } from "@earendil-works/pi-ai/providers/openrouter-api";

const MODELS_URL = `${OPENROUTER_BASE_URL["openai-completions"]}/models`;
const REQUEST_TIMEOUT_MS = 5_000;
const MAX_MODELS_BYTES = 8 * 1024 * 1024;
const MODEL_ID = /^[A-Za-z0-9~][A-Za-z0-9._:~/-]{0,159}$/u;
const THINKING_LEVELS = ["off", "minimal", "low", "medium", "high", "xhigh", "max"] as const;

export const OPENROUTER_SOURCE: LiveCatalogSource = {
  provider: "openrouter",
  maxModels: 2_048,
  isValidId: (value: unknown): value is string => typeof value === "string" && MODEL_ID.test(value),
};

export interface OpenRouterCatalogOptions {
  readonly cachePath?: string;
  readonly fetch?: typeof globalThis.fetch;
  readonly signal?: AbortSignal;
  readonly timeoutMs?: number;
  readonly warn?: (message: string) => void;
}

/**
 * Pi's bundled OpenRouter catalog, plus the tool-capable chat models
 * OpenRouter lists that Pi does not know yet. A learned model keeps its first
 * binding; the last good list stays in use while OpenRouter is unreachable.
 */
export async function loadOpenRouterCatalog(
  authStorePath: string,
  pinned: readonly CatalogEntry[],
  options: OpenRouterCatalogOptions = {},
): Promise<readonly CatalogEntry[]> {
  const cachePath = options.cachePath ?? join(dirname(authStorePath), "openrouter-catalog-v1.json");
  const cached = await readLiveCatalogCache(cachePath, OPENROUTER_SOURCE, pinned);
  const signal = requestSignal(options.signal, options.timeoutMs ?? REQUEST_TIMEOUT_MS, "OpenRouter");
  let live: { readonly ids: string[]; readonly models: Model<Api>[] };
  try {
    live = projectModels(await fetchModels(options.fetch ?? globalThis.fetch, signal), pinned);
  } catch (error) {
    warn(options, `OpenRouter model list refresh failed: ${message(error)}`);
    return cached === undefined
      ? pinned
      : selectAvailableModels(pinned, cached.dynamic_models, cached.available_model_ids);
  }
  const dynamicModels = mergeLearnedModels(cached?.dynamic_models ?? [], live.models);
  try {
    await writeLiveCatalogCache(cachePath, {
      available_model_ids: live.ids,
      dynamic_models: dynamicModels,
    });
  } catch (error) {
    warn(options, `OpenRouter catalog cache could not be updated: ${message(error)}`);
  }
  return selectAvailableModels(pinned, dynamicModels, live.ids);
}

async function fetchModels(fetcher: typeof globalThis.fetch, signal: AbortSignal): Promise<unknown> {
  const response = await fetcher(MODELS_URL, {
    headers: { accept: "application/json" },
    redirect: "error",
    signal,
  });
  if (response.status !== 200) {
    throw new Error(`model list returned HTTP ${response.status}`);
  }
  return readBoundedJson(response, MAX_MODELS_BYTES);
}

function projectModels(
  body: unknown,
  pinned: readonly CatalogEntry[],
): { readonly ids: string[]; readonly models: Model<Api>[] } {
  const data = isRecord(body) ? body.data : undefined;
  if (!Array.isArray(data) || data.length === 0 || data.length > OPENROUTER_SOURCE.maxModels) {
    throw new Error("model list is invalid");
  }
  const pinnedIds = new Set(pinned.map((entry) => entry.id));
  const ids = new Set<string>();
  const models: Model<Api>[] = [];
  for (const value of data) {
    const id = isRecord(value) ? value.id : undefined;
    if (!isRecord(value) || !OPENROUTER_SOURCE.isValidId(id) || !ids.add(id)) {
      throw new Error("model list has an invalid or repeated model id");
    }
    if (pinnedIds.has(id)) {
      continue;
    }
    const model = projectModel(id, value);
    if (model !== undefined) {
      models.push(validateModelSpec(model, "openrouter", id));
    }
  }
  return { ids: [...ids].sort(), models };
}

/**
 * A chat model that reads and writes text and accepts tools, on Chat
 * Completions. Pi's OpenRouter request rules (reasoning object, roles, session
 * affinity) come from the provider and base URL, so no compat block is set.
 */
function projectModel(id: string, value: Record<string, unknown>): Record<string, unknown> | undefined {
  const name = value.name;
  const architecture = isRecord(value.architecture) ? value.architecture : undefined;
  const input = architecture?.input_modalities;
  const output = architecture?.output_modalities;
  const parameters = Array.isArray(value.supported_parameters) ? value.supported_parameters : [];
  const top = isRecord(value.top_provider) ? value.top_provider : undefined;
  const contextWindow = top?.context_length ?? value.context_length;
  const maxTokens = top?.max_completion_tokens;
  if (
    typeof name !== "string" ||
    name.length === 0 ||
    name.length > 200 ||
    !Array.isArray(input) ||
    !input.includes("text") ||
    !Array.isArray(output) ||
    !output.includes("text") ||
    !parameters.includes("tools") ||
    !isPositiveSafeInteger(contextWindow) ||
    !isPositiveSafeInteger(maxTokens)
  ) {
    return undefined;
  }
  const reasoning = parameters.includes("reasoning");
  const thinkingLevelMap = reasoning ? projectThinkingLevels(value.reasoning) : undefined;
  return {
    id,
    name,
    api: "openai-completions",
    provider: "openrouter",
    baseUrl: OPENROUTER_BASE_URL["openai-completions"],
    reasoning,
    input: input.includes("image") ? ["text", "image"] : ["text"],
    cost: openRouterCost(isRecord(value.pricing) ? (value.pricing as OpenRouterPricing) : undefined),
    contextWindow,
    maxTokens,
    ...(thinkingLevelMap === undefined ? {} : { thinkingLevelMap }),
  };
}

/**
 * OpenRouter lists the efforts a model accepts and whether reasoning can be
 * turned off. A model with no effort list keeps Pi's default levels, without
 * `off` when its reasoning is mandatory.
 */
function projectThinkingLevels(value: unknown): ThinkingLevelMap | undefined {
  const reasoning = isRecord(value) ? value : {};
  const mandatory = reasoning.mandatory === true;
  const efforts = reasoning.supported_efforts;
  if (!Array.isArray(efforts)) {
    return mandatory ? { off: null } : undefined;
  }
  const map: ThinkingLevelMap = Object.fromEntries(THINKING_LEVELS.map((level) => [level, null]));
  let supported = 0;
  for (const effort of efforts) {
    const level = effort === "none" ? "off" : effort;
    if (typeof level === "string" && (THINKING_LEVELS as readonly string[]).includes(level)) {
      map[level as keyof ThinkingLevelMap] = effort as string;
      supported += 1;
    }
  }
  if (supported === 0) {
    return mandatory ? { off: null } : undefined;
  }
  if (!mandatory) {
    map.off = "none";
  }
  return map;
}

function warn(options: OpenRouterCatalogOptions, value: string): void {
  (options.warn ?? console.error)(value);
}

function message(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function isPositiveSafeInteger(value: unknown): value is number {
  return Number.isSafeInteger(value) && (value as number) > 0;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
