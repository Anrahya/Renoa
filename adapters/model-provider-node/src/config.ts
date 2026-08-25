import { isAbsolute } from "node:path";

import type { ProviderId, ReasoningLevel } from "./contract.js";

export interface BridgeConfig {
  readonly action: "catalog" | "describe" | "stream";
  readonly provider: ProviderId;
  readonly authStorePath: string;
  readonly modelId?: string;
  readonly modelSpec?: unknown;
  readonly reasoningLevel?: ReasoningLevel;
  readonly maxOutputTokens?: number;
  readonly allowLoopback: boolean;
}

export function loadBridgeConfig(environment: NodeJS.ProcessEnv): BridgeConfig {
  const action = requiredAction(environment.RENOA_MODEL_ACTION);
  const provider = requiredProvider(environment.RENOA_MODEL_PROVIDER);
  const allowLoopback = environment.RENOA_MODEL_ALLOW_LOOPBACK === "1";
  const base = {
    action,
    provider,
    authStorePath: absolute(environment, "RENOA_MODEL_AUTH_STORE"),
    allowLoopback,
  };
  if (action === "catalog") {
    return base;
  }
  const modelId = required(environment, "RENOA_MODEL");
  const modelSpec = optionalJson(environment.RENOA_MODEL_SPEC, "RENOA_MODEL_SPEC");
  const reasoningLevel = optionalReasoning(environment.RENOA_MODEL_REASONING);
  return {
    ...base,
    modelId,
    ...(modelSpec === undefined ? {} : { modelSpec }),
    ...(reasoningLevel === undefined ? {} : { reasoningLevel }),
    ...(action === "stream"
      ? {
          maxOutputTokens: positiveInteger(
            environment.RENOA_MODEL_MAX_OUTPUT_TOKENS,
            "RENOA_MODEL_MAX_OUTPUT_TOKENS",
          ),
        }
      : {}),
  };
}

export function loadAuthStorePath(environment: NodeJS.ProcessEnv): string {
  return absolute(environment, "RENOA_MODEL_AUTH_STORE");
}

function requiredAction(value: string | undefined): BridgeConfig["action"] {
  if (value === "catalog" || value === "describe" || value === "stream") {
    return value;
  }
  throw new Error("RENOA_MODEL_ACTION must be catalog, describe, or stream");
}

function requiredProvider(value: string | undefined): ProviderId {
  if (value === "xai" || value === "opencode-go") {
    return value;
  }
  throw new Error("RENOA_MODEL_PROVIDER must be opencode-go or xai");
}

function optionalJson(value: string | undefined, name: string): unknown {
  if (value === undefined) {
    return undefined;
  }
  try {
    return JSON.parse(value) as unknown;
  } catch {
    throw new Error(`${name} must be valid JSON`);
  }
}

function optionalReasoning(value: string | undefined): ReasoningLevel | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (["off", "minimal", "low", "medium", "high", "xhigh", "max"].includes(value)) {
    return value as ReasoningLevel;
  }
  throw new Error("RENOA_MODEL_REASONING is invalid");
}

function positiveInteger(value: string | undefined, name: string): number {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return parsed;
}

function absolute(environment: NodeJS.ProcessEnv, name: string): string {
  const value = required(environment, name);
  if (!isAbsolute(value)) {
    throw new Error(`${name} must be absolute`);
  }
  return value;
}

function required(environment: NodeJS.ProcessEnv, name: string): string {
  const value = environment[name];
  if (value === undefined || value === "") {
    throw new Error(`${name} is required`);
  }
  return value;
}
