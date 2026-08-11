import type { StreamFn } from "@earendil-works/pi-agent-core";
import { createModels, type Api, type Model, type Provider } from "@earendil-works/pi-ai";
import { opencodeGoProvider } from "@earendil-works/pi-ai/providers/opencode-go";
import { xaiProvider } from "@earendil-works/pi-ai/providers/xai";

import type { PiProvider } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";

export interface ModelRuntimeOptions {
  readonly provider: PiProvider;
  readonly modelId: string;
  readonly authStorePath: string;
}

export interface ModelRuntime {
  readonly model: Model<Api>;
  readonly streamFn: StreamFn;
  close(): void;
}

export async function loadModelRuntime(options: ModelRuntimeOptions): Promise<ModelRuntime> {
  const credentials = new SqliteCredentialStore(options.authStorePath);
  try {
    const models = createModels({ credentials });
    models.setProvider(createProvider(options.provider));
    const model = models.getModel(options.provider, options.modelId);
    if (model === undefined) {
      const available = models
        .getModels(options.provider)
        .map((candidate) => candidate.id)
        .join(", ");
      throw new Error(
        `unknown ${options.provider} model ${options.modelId}; available models: ${available}`,
      );
    }
    if ((await models.checkAuth(options.provider)) === undefined) {
      throw new Error(`${options.provider} credentials are not configured`);
    }
    return {
      model,
      streamFn: models.streamSimple.bind(models),
      close: () => credentials.close(),
    };
  } catch (error) {
    credentials.close();
    throw error;
  }
}

function createProvider(provider: PiProvider): Provider {
  switch (provider) {
    case "opencode-go":
      return opencodeGoProvider();
    case "xai":
      return xaiProvider();
  }
}
