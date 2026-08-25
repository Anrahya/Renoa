import type { DescriptionResponse, WireModelRequest, WireStreamRecord } from "./contract.js";
import { parseWireModelRequest } from "./wire-request.js";
import type { BridgeConfig } from "./config.js";
import { loadCatalog, loadRuntime } from "./runtime.js";
import { streamModel, wireError } from "./stream.js";

export async function runBridgeAction(
  config: BridgeConfig,
  request: WireModelRequest | undefined,
  emit: (record: DescriptionResult | WireStreamRecord) => Promise<void>,
  signal: AbortSignal,
): Promise<void> {
  if (config.action === "catalog") {
    await emit({
      ok: true,
      response: { models: loadCatalog(config.provider, config.authStorePath) },
    });
    return;
  }
  const streamRequest = config.action === "stream" ? requiredRequest(request) : undefined;
  const runtime = loadRuntime({
    provider: config.provider,
    modelId: requiredModelId(config),
    authStorePath: config.authStorePath,
    ...(config.modelSpec === undefined ? {} : { modelSpec: config.modelSpec }),
    ...(config.reasoningLevel === undefined ? {} : { reasoningLevel: config.reasoningLevel }),
    ...(config.allowLoopback ? { allowLoopback: true } : {}),
  });
  try {
    if (config.action === "describe") {
      await emit({
        ok: true,
        response: {
          context_window_tokens: runtime.model.contextWindow,
          max_output_tokens: runtime.model.maxTokens,
          model_binding_id: runtime.modelBindingId,
          model_spec: runtime.modelSpec,
          reasoning_level: runtime.reasoningLevel,
        } satisfies DescriptionResponse,
      });
      return;
    }
    if (streamRequest === undefined) {
      throw Object.assign(new Error("model stream request is required on standard input"), {
        categoryHint: "invalid_request",
      });
    }
    try {
      await streamModel({
        runtime,
        request: streamRequest,
        maxOutputTokens: requiredMaxOutput(config),
        signal,
        emit,
      });
    } catch (error) {
      await emit(wireError(error, { provider: runtime.provider, model: runtime.model.id }));
    }
  } finally {
    runtime.close();
  }
}

export interface DescriptionResult {
  readonly ok: true;
  readonly response: unknown;
}

function requiredModelId(config: BridgeConfig): string {
  if (config.modelId === undefined || config.modelId.length === 0) {
    throw new Error("RENOA_MODEL is required");
  }
  return config.modelId;
}

function requiredRequest(request: WireModelRequest | undefined): WireModelRequest {
  if (request === undefined) {
    throw Object.assign(new Error("model stream request is required on standard input"), {
      categoryHint: "invalid_request",
    });
  }
  return parseWireModelRequest(request);
}

function requiredMaxOutput(config: BridgeConfig): number {
  if (config.maxOutputTokens === undefined) {
    throw new Error("RENOA_MODEL_MAX_OUTPUT_TOKENS is required");
  }
  return config.maxOutputTokens;
}
