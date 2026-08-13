import { createHash } from "node:crypto";

import type { StreamFn } from "@earendil-works/pi-agent-core";
import { createModels, type Api, type Model, type Provider } from "@earendil-works/pi-ai";
import { opencodeGoProvider } from "@earendil-works/pi-ai/providers/opencode-go";
import { xaiProvider } from "@earendil-works/pi-ai/providers/xai";

import type { PiProvider } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";

const DEFAULT_CATALOG_BASE_URL = "https://pi.dev";
const CATALOG_TIMEOUT_MS = 10_000;
const RETRYABLE_CATALOG_STATUSES = new Set([408, 425, 429, 500, 502, 503, 504]);

export interface ModelRuntimeOptions {
  readonly provider: PiProvider;
  readonly modelId: string;
  readonly authStorePath: string;
  readonly catalogBaseUrl?: string;
  readonly modelSpec?: unknown;
}

export interface ModelRuntime {
  readonly model: Model<Api>;
  readonly modelBindingId: string;
  readonly modelSpec: string;
  readonly streamFn: StreamFn;
  close(): void;
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
    return {
      model,
      modelBindingId: modelBindingId(model),
      modelSpec: JSON.stringify(model),
      streamFn: models.streamSimple.bind(models),
      close: () => credentials.close(),
    };
  } catch (error) {
    credentials.close();
    throw error;
  }
}

async function loadRemoteModel(
  provider: PiProvider,
  modelId: string,
  catalogBaseUrl: string,
): Promise<Model<Api> | undefined> {
  if (provider !== "xai") {
    return undefined;
  }
  const endpoint = new URL(`/api/models/providers/${provider}`, catalogBaseUrl);
  let response: Response;
  try {
    response = await fetchCatalog(endpoint);
  } catch (error) {
    const detail = error instanceof Error ? error.message : String(error);
    throw new Error(`Pi model catalog request failed for ${provider}: ${detail}`);
  }
  if (response.status === 404 || response.status === 501) {
    return undefined;
  }
  if (!response.ok) {
    throw new Error(`Pi model catalog request failed for ${provider}: ${response.status}`);
  }
  const candidate = catalogEntries(await response.json()).find(
    (entry): entry is Record<string, unknown> => isRecord(entry) && entry.id === modelId,
  );
  return candidate === undefined ? undefined : validateXaiModel(candidate, modelId);
}

async function fetchCatalog(endpoint: URL): Promise<Response> {
  try {
    const first = await catalogFetch(endpoint);
    if (!RETRYABLE_CATALOG_STATUSES.has(first.status)) {
      return first;
    }
    await first.body?.cancel();
  } catch {
    // One bounded retry covers a cold or transient discovery connection.
  }
  return catalogFetch(endpoint);
}

function catalogFetch(endpoint: URL): Promise<Response> {
  return fetch(endpoint, {
    headers: { accept: "application/json", "user-agent": "Renoa/0.1" },
    signal: AbortSignal.timeout(CATALOG_TIMEOUT_MS),
  });
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
    !trustedBaseUrl(provider, value.baseUrl) ||
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

function trustedBaseUrl(provider: PiProvider, value: unknown): boolean {
  if (typeof value !== "string") {
    return false;
  }
  if (provider === "xai") {
    return value === "https://api.x.ai/v1";
  }
  return value === "https://opencode.ai/zen/go";
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
