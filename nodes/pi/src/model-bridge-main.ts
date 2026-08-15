import { loadModelConfig, loadProviderConfig } from "./config.js";
import {
  ModelInvocationError,
  streamModel,
  type WireModelRequest,
  type WireStreamRecord,
} from "./model-bridge.js";
import { loadModelCatalog, loadModelRuntime } from "./model-runtime.js";

interface DescriptionResult {
  readonly ok: true;
  readonly response: unknown;
}

async function main(): Promise<void> {
  const action = requiredAction(process.env.RENOA_PI_ACTION);
  if (action === "catalog") {
    await writeRecord({
      ok: true,
      response: { models: await loadModelCatalog(loadProviderConfig(process.env)) },
    });
    return;
  }
  const reasoningLevel = optionalReasoning(process.env.RENOA_PI_REASONING);
  const runtime = await loadModelRuntime({
    ...loadModelConfig(process.env),
    modelSpec: optionalJson(process.env.RENOA_PI_MODEL_SPEC),
    ...(reasoningLevel === undefined ? {} : { reasoningLevel }),
  });
  try {
    switch (action) {
      case "describe":
        await writeRecord({
          ok: true,
          response: {
            context_window_tokens: runtime.model.contextWindow,
            max_output_tokens: runtime.model.maxTokens,
            model_binding_id: runtime.modelBindingId,
            model_spec: runtime.modelSpec,
            reasoning_level: runtime.reasoningLevel,
          },
        });
        return;
      case "stream": {
        const { request, maxOutputTokens } = await readInvocation();
        try {
          await streamModel(request, runtime, maxOutputTokens, writeRecord);
        } catch (error) {
          await writeRecord(streamFailure(error));
        }
        return;
      }
    }
  } finally {
    runtime.close();
  }
}

async function readInvocation(): Promise<{
  request: WireModelRequest;
  maxOutputTokens: number;
}> {
  const input = await readStdin();
  return {
    request: JSON.parse(input) as WireModelRequest,
    maxOutputTokens: positiveInteger(
      process.env.RENOA_PI_MAX_OUTPUT_TOKENS,
      "RENOA_PI_MAX_OUTPUT_TOKENS",
    ),
  };
}

function optionalJson(value: string | undefined): unknown {
  if (value === undefined) {
    return undefined;
  }
  try {
    return JSON.parse(value) as unknown;
  } catch {
    throw new Error("RENOA_PI_MODEL_SPEC must be valid JSON");
  }
}

function optionalReasoning(
  value: string | undefined,
): "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max" | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (["off", "minimal", "low", "medium", "high", "xhigh", "max"].includes(value)) {
    return value as "off" | "minimal" | "low" | "medium" | "high" | "xhigh" | "max";
  }
  throw new Error("RENOA_PI_REASONING is invalid");
}

function requiredAction(value: string | undefined): "catalog" | "describe" | "stream" {
  if (value === "catalog" || value === "describe" || value === "stream") {
    return value;
  }
  throw new Error("RENOA_PI_ACTION must be catalog, describe, or stream");
}

function positiveInteger(value: string | undefined, name: string): number {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return parsed;
}

function streamFailure(error: unknown): WireStreamRecord {
  return {
    event: "error",
    error: error instanceof Error ? error.message : String(error),
    ...(error instanceof ModelInvocationError && error.kind !== undefined
      ? { error_kind: error.kind }
      : {}),
  };
}

function writeRecord(record: DescriptionResult | WireStreamRecord): Promise<void> {
  return new Promise((resolve, reject) => {
    process.stdout.write(`${JSON.stringify(record)}\n`, (error) => {
      if (error === null || error === undefined) {
        resolve();
      } else {
        reject(error);
      }
    });
  });
}

async function readStdin(): Promise<string> {
  process.stdin.setEncoding("utf8");
  let input = "";
  for await (const chunk of process.stdin) {
    input += chunk;
  }
  return input;
}

main().catch((error: unknown) => {
  process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  process.exitCode = 1;
});
