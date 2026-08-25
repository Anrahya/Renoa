import type { ProviderId, ReasoningLevel } from "./contract.js";
import {
  SqliteCredentialStore,
  type Credential,
  type CredentialStoreOptions,
} from "./credentials.js";
import {
  findCatalogModel,
  loadPinnedCatalog,
  modelBindingId,
  resolveReasoningLevel,
  toWireCatalog,
  validateModelSpec,
  type CatalogEntry,
} from "./catalog.js";
import { fromOauth, oauthCredential, xaiOAuth } from "./providers/xai.js";
import type { Api, Model, SimpleStreamOptions } from "./upstream/types.js";
import { streamSimple as streamAnthropic } from "./transports/anthropic-messages.js";
import { streamSimple as streamOpenAiChat } from "./transports/openai-chat.js";
import { streamSimple as streamOpenAiResponses } from "./transports/openai-responses.js";

const OAUTH_SKEW_MS = 5 * 60 * 1000;

export interface RuntimeOptions {
  readonly provider: ProviderId;
  readonly modelId: string;
  readonly authStorePath: string;
  readonly modelSpec?: unknown;
  readonly reasoningLevel?: ReasoningLevel;
  readonly allowLoopback?: boolean;
  readonly credentialStore?: CredentialStoreOptions;
}

export interface LoadedRuntime {
  readonly provider: ProviderId;
  readonly model: Model<Api>;
  readonly modelBindingId: string;
  readonly modelSpec: string;
  readonly reasoningLevel: ReasoningLevel;
  readonly credentials: SqliteCredentialStore;
  close(): void;
}

export function loadCatalog(
  provider: ProviderId,
  authStorePath: string,
  options: CredentialStoreOptions = {},
) {
  const credentials = new SqliteCredentialStore(authStorePath, options);
  try {
    requireCredential(credentials, provider);
    return toWireCatalog(loadPinnedCatalog(provider));
  } finally {
    credentials.close();
  }
}

export function loadRuntime(options: RuntimeOptions): LoadedRuntime {
  const credentials = new SqliteCredentialStore(options.authStorePath, options.credentialStore);
  try {
    requireCredential(credentials, options.provider);
    const model = resolveModel(options);
    const reasoningLevel = resolveReasoningLevel(model, options.reasoningLevel);
    return {
      provider: options.provider,
      model,
      modelBindingId: modelBindingId(model),
      modelSpec: JSON.stringify(model),
      reasoningLevel,
      credentials,
      close: () => credentials.close(),
    };
  } catch (error) {
    credentials.close();
    throw error;
  }
}

export async function resolveApiKey(
  runtime: LoadedRuntime,
  signal: AbortSignal,
): Promise<string> {
  const credential = requireCredential(runtime.credentials, runtime.provider);
  if (credential.type === "api_key") {
    if (typeof credential.key !== "string" || credential.key.length === 0) {
      throw new Error(`${runtime.provider} credentials are not configured`);
    }
    return credential.key;
  }
  if (credential.expires > Date.now() + OAUTH_SKEW_MS) {
    return credential.access;
  }
  const refreshed = await refreshOauth(runtime, credential, signal);
  return refreshed.access;
}

export async function refreshOauth(
  runtime: LoadedRuntime,
  current: Extract<Credential, { type: "oauth" }>,
  signal: AbortSignal,
): Promise<Extract<Credential, { type: "oauth" }>> {
  if (runtime.provider !== "xai") {
    throw new Error(`${runtime.provider} does not support OAuth refresh`);
  }
  const next = await runtime.credentials.refreshOauth(runtime.provider, async (snapshot) => {
    const refreshed = await xaiOAuth.refresh(oauthCredential(snapshot), signal);
    return fromOauth(refreshed);
  });
  if (next === undefined || next.type !== "oauth") {
    throw new Error("xAI OAuth refresh did not produce a stored credential");
  }
  // If another process won the compare-and-store race, use whatever is stored.
  if (next.access === current.access && next.expires <= Date.now()) {
    throw new Error("xAI OAuth refresh did not replace the expired access token");
  }
  return next;
}

export function dispatchStream(
  model: Model<Api>,
  context: Parameters<typeof streamOpenAiChat>[1],
  options: SimpleStreamOptions,
) {
  switch (model.api) {
    case "openai-completions":
      return streamOpenAiChat(model as Model<"openai-completions">, context, options);
    case "openai-responses":
      return streamOpenAiResponses(model as Model<"openai-responses">, context, options);
    case "anthropic-messages":
      return streamAnthropic(model as Model<"anthropic-messages">, context, options);
  }
}

function resolveModel(options: RuntimeOptions): Model<Api> {
  if (options.modelSpec !== undefined) {
    return validateModelSpec(options.modelSpec, options.provider, options.modelId, {
      ...(options.allowLoopback === true ? { allowLoopback: true } : {}),
    });
  }
  const found: CatalogEntry | undefined = findCatalogModel(options.provider, options.modelId);
  if (found === undefined) {
    const available = loadPinnedCatalog(options.provider)
      .map((entry) => entry.id)
      .join(", ");
    throw new Error(
      `unknown ${options.provider} model ${options.modelId}; available models: ${available}`,
    );
  }
  return found.model;
}

function requireCredential(
  store: SqliteCredentialStore,
  provider: ProviderId,
): Credential {
  const credential = store.read(provider);
  if (credential === undefined) {
    throw new Error(`${provider} credentials are not configured`);
  }
  if (provider === "xai" && credential.type !== "oauth") {
    throw new Error("xAI requires OAuth credentials");
  }
  if (provider === "opencode-go" && credential.type !== "api_key") {
    throw new Error("OpenCode Go requires an API key");
  }
  return credential;
}
