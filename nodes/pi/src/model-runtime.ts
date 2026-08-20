import { createHash } from "node:crypto";

import type { StreamFn } from "@earendil-works/pi-agent-core";
import {
  createModels,
  getSupportedThinkingLevels,
  ModelsError,
  type Api,
  type Model,
  type ModelThinkingLevel,
  type Provider,
} from "@earendil-works/pi-ai";
import { opencodeGoProvider } from "@earendil-works/pi-ai/providers/opencode-go";
import { xaiProvider } from "@earendil-works/pi-ai/providers/xai";

import type { PiProvider } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";

const DEFAULT_CATALOG_BASE_URL = "https://pi.dev";
const DISCOVERY_TIMEOUT_MS = 2_000;
const RESOLUTION_TIMEOUT_MS = 10_000;

export interface ModelRuntimeOptions {
  readonly provider: PiProvider;
  readonly modelId: string;
  readonly authStorePath: string;
  readonly catalogBaseUrl?: string;
  readonly modelSpec?: unknown;
  readonly reasoningLevel?: ModelThinkingLevel;
}

export interface ModelRuntime {
  readonly model: Model<Api>;
  readonly modelBindingId: string;
  readonly modelSpec: string;
  readonly reasoningLevel: ModelThinkingLevel;
  readonly authenticate: () => Promise<void>;
  readonly streamFn: StreamFn;
  close(): void;
}

export interface ModelCatalogEntry {
  readonly id: string;
  readonly name: string;
  readonly reasoning_levels: readonly string[];
  readonly model_spec: Model<Api>;
}

export interface ModelCatalogOptions {
  readonly provider: PiProvider;
  readonly authStorePath: string;
  readonly catalogBaseUrl?: string;
}

export async function loadModelCatalog(
  options: ModelCatalogOptions,
): Promise<readonly ModelCatalogEntry[]> {
  const credentials = new SqliteCredentialStore(options.authStorePath);
  try {
    const models = createModels({ credentials });
    const provider = createProvider(options.provider);
    models.setProvider(provider);
    if ((await models.checkAuth(options.provider)) === undefined) {
      throw new Error(`${options.provider} credentials are not configured`);
    }
    const discovered = [...models.getModels(options.provider)];
    if (options.provider === "xai") {
      try {
        for (const model of await loadRemoteModels(
          options.provider,
          options.catalogBaseUrl ?? DEFAULT_CATALOG_BASE_URL,
        )) {
          const existing = discovered.findIndex((candidate) => candidate.id === model.id);
          if (existing === -1) {
            discovered.push(model);
          } else {
            discovered[existing] = model;
          }
        }
      } catch {
        // The package-pinned catalog remains usable when live discovery is unavailable.
      }
    }
    return discovered
      .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id))
      .map((model) => ({
        id: model.id,
        name: model.name,
        reasoning_levels: getSupportedThinkingLevels(model),
        model_spec: model,
      }));
  } finally {
    credentials.close();
  }
}

export async function loadModelRuntime(options: ModelRuntimeOptions): Promise<ModelRuntime> {
  const credentials = new SqliteCredentialStore(options.authStorePath);
  try {
    const models = createModels({ credentials });
    const provider = createProvider(options.provider);
    models.setProvider(provider);
    if ((await models.checkAuth(options.provider)) === undefined) {
      throw new Error(`${options.provider} credentials are not configured`);
    }
    let model: Model<Api> | undefined;
    if (options.modelSpec === undefined) {
      model = models.getModel(options.provider, options.modelId);
    } else {
      model = validateModelSpec(options.modelSpec, options.provider, options.modelId);
      models.setProvider(withModel(provider, model));
    }
    if (model === undefined) {
      const remote = await loadRemoteModel(
        options.provider,
        options.modelId,
        options.catalogBaseUrl ?? DEFAULT_CATALOG_BASE_URL,
      );
      if (remote !== undefined) {
        models.setProvider(withModel(provider, remote));
        model = models.getModel(options.provider, options.modelId);
      }
    }
    if (model === undefined) {
      const available = models
        .getModels(options.provider)
        .map((candidate) => candidate.id)
        .join(", ");
      throw new Error(
        `unknown ${options.provider} model ${options.modelId}; available models: ${available}`,
      );
    }
    const selectedModel = model;
    const reasoningLevel = resolveReasoningLevel(selectedModel, options.reasoningLevel);
    return {
      model: selectedModel,
      modelBindingId: modelBindingId(selectedModel),
      modelSpec: JSON.stringify(selectedModel),
      reasoningLevel,
      authenticate: async () => {
        if ((await models.getAuth(selectedModel)) === undefined) {
          throw new ModelsError(
            "auth",
            `Provider is not configured: ${selectedModel.provider}`,
          );
        }
      },
      streamFn: models.streamSimple.bind(models),
      close: () => credentials.close(),
    };
  } catch (error) {
    credentials.close();
    throw error;
  }
}

function resolveReasoningLevel(
  model: Model<Api>,
  requested: ModelThinkingLevel | undefined,
): ModelThinkingLevel {
  const supported = getSupportedThinkingLevels(model);
  if (requested !== undefined) {
    if (!supported.includes(requested)) {
      throw new Error(`${model.id} does not support ${requested} reasoning`);
    }
    return requested;
  }
  return supported.includes("high") ? "high" : (supported[0] ?? "off");
}

async function loadRemoteModel(
  provider: PiProvider,
  modelId: string,
  catalogBaseUrl: string,
): Promise<Model<Api> | undefined> {
  if (provider !== "xai") {
    return undefined;
  }
  const entries = await loadRemoteEntries(provider, catalogBaseUrl, RESOLUTION_TIMEOUT_MS);
  const candidate = entries.find((entry) => entry.id === modelId);
  return candidate === undefined ? undefined : validateXaiModel(candidate, modelId);
}

async function loadRemoteModels(
  provider: PiProvider,
  catalogBaseUrl: string,
): Promise<Model<Api>[]> {
  const entries = await loadRemoteEntries(provider, catalogBaseUrl, DISCOVERY_TIMEOUT_MS);
  return entries.flatMap((entry) => {
    try {
      return [validateXaiModel(entry, String(entry.id))];
    } catch {
      return [];
    }
  });
}

async function loadRemoteEntries(
  provider: PiProvider,
  catalogBaseUrl: string,
  timeoutMs: number,
): Promise<Record<string, unknown>[]> {
  const endpoint = new URL(`/api/models/providers/${provider}`, catalogBaseUrl);
  let response: Response;
  try {
    response = await fetch(endpoint, {
      headers: { accept: "application/json", "user-agent": "Renoa/0.1" },
      signal: AbortSignal.timeout(timeoutMs),
    });
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`Pi model catalog request failed for ${provider}: ${detail}`);
  }
  if (response.status === 404 || response.status === 501) {
    return [];
  }
  if (!response.ok) {
    throw new Error(`Pi model catalog request failed for ${provider}: ${response.status}`);
  }
  return catalogEntries(await response.json())
    .filter((entry): entry is Record<string, unknown> => isRecord(entry));
}

function catalogEntries(value: unknown): readonly unknown[] {
  if (Array.isArray(value)) {
    return value;
  }
  if (isRecord(value) && Array.isArray(value.models)) {
    return value.models;
  }
  return isRecord(value) ? Object.values(value) : [];
}

function validateXaiModel(value: Record<string, unknown>, modelId: string): Model<Api> {
  return validateModelSpec(value, "xai", modelId);
}

function validateModelSpec(
  value: unknown,
  provider: PiProvider,
  modelId: string,
): Model<Api> {
  if (!isRecord(value)) {
    throw new Error(`Pi model binding for ${provider}/${modelId} is invalid`);
  }
  const cost = value.cost;
  if (
    value.id !== modelId ||
    typeof value.name !== "string" ||
    value.name.length === 0 ||
    !supportsApi(provider, value.api) ||
    value.provider !== provider ||
    !trustedBaseUrl(provider, value.api, value.baseUrl) ||
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
    throw new Error(`Pi model binding for ${provider}/${modelId} is invalid`);
  }
  return structuredClone(value) as unknown as Model<Api>;
}

function supportsApi(provider: PiProvider, api: unknown): boolean {
  return provider === "xai"
    ? api === "openai-completions" || api === "openai-responses"
    : api === "anthropic-messages" || api === "openai-completions" || api === "openai-responses";
}

function trustedBaseUrl(provider: PiProvider, api: unknown, value: unknown): boolean {
  if (typeof value !== "string") {
    return false;
  }
  if (provider === "xai") {
    return value === "https://api.x.ai/v1";
  }
  return api === "anthropic-messages"
    ? value === "https://opencode.ai/zen/go"
    : value === "https://opencode.ai/zen/go/v1";
}

function modelBindingId(model: Model<Api>): string {
  return createHash("sha256").update(JSON.stringify(model)).digest("hex");
}

function withModel(provider: Provider, model: Model<Api>): Provider {
  const models = provider.getModels().filter((candidate) => candidate.id !== model.id);
  return { ...provider, getModels: () => [...models, model] };
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

function createProvider(provider: PiProvider): Provider {
  switch (provider) {
    case "opencode-go":
      return opencodeGoProvider();
    case "xai":
      return xaiProvider();
  }
}
