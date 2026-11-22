import { readFileSync } from "node:fs";
import { dirname, join } from "node:path";

import type { Api, Model } from "@earendil-works/pi-ai";

/**
 * OpenRouter's endpoints, as Pi `packages/ai/src/providers/openrouter.ts`
 * (v1.5.0) configures them: Chat Completions under `/api/v1`, and Anthropic
 * Messages under `/api` for the Claude entries in Pi's catalog.
 */
export const OPENROUTER_BASE_URL = {
  "openai-completions": "https://openrouter.ai/api/v1",
  "anthropic-messages": "https://openrouter.ai/api",
} as const;

/** The owner's OpenRouter provider preferences, beside the credential store. */
const OPENROUTER_ROUTING_FILE = "openrouter-routing.json";

/**
 * Applies the owner's provider preferences for this model, if any, as Pi's
 * `compat.openRouterRouting`, which becomes the request's `provider` object.
 * The file maps a model id to OpenRouter's provider preferences, for example
 * `{"thinkingmachines/inkling-2-small": {"order": ["primeintellect"]}}`.
 * It is read on every load, so a change reaches the next model call. The
 * caller binds the model before this, so a preference never changes a binding.
 */
export function withOpenRouterRouting(model: Model<Api>, authStorePath: string): Model<Api> {
  const path = join(dirname(authStorePath), OPENROUTER_ROUTING_FILE);
  let text: string;
  try {
    text = readFileSync(path, "utf8");
  } catch (error) {
    if (error instanceof Error && "code" in error && error.code === "ENOENT") {
      return model;
    }
    throw error;
  }
  const invalid = `${path} must map OpenRouter model ids to provider preference objects`;
  let preferences: unknown;
  try {
    preferences = JSON.parse(text) as unknown;
  } catch {
    throw new Error(invalid);
  }
  if (!isRecord(preferences) || !Object.values(preferences).every(isRecord)) {
    throw new Error(invalid);
  }
  const routing = preferences[model.id];
  if (routing === undefined) {
    return model;
  }
  if (model.api !== "openai-completions") {
    throw new Error(
      `${path} sets provider preferences for ${model.id}, but Renoa sends them only on Chat Completions and ${model.id} uses ${model.api}`,
    );
  }
  return { ...model, compat: { ...model.compat, openRouterRouting: routing } } as Model<Api>;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
