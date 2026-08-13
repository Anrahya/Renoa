import { loadModelConfig } from "./config.js";
import {
  ModelInvocationError,
  invokeModel,
  type WireModelRequest,
} from "./model-bridge.js";
import { loadModelRuntime } from "./model-runtime.js";

interface BridgeResult {
  readonly ok: boolean;
  readonly response?: unknown;
  readonly error?: string;
  readonly error_kind?: "context_window_exceeded";
}

async function main(): Promise<BridgeResult> {
  const runtime = await loadModelRuntime({
    ...loadModelConfig(process.env),
    modelSpec: optionalJson(process.env.RENOA_PI_MODEL_SPEC),
  });
  try {
    switch (requiredAction(process.env.RENOA_PI_ACTION)) {
      case "describe":
        return {
          ok: true,
          response: {
            context_window_tokens: runtime.model.contextWindow,
            max_output_tokens: runtime.model.maxTokens,
            model_binding_id: runtime.modelBindingId,
            model_spec: runtime.modelSpec,
          },
        };
      case "invoke": {
        const input = await readStdin();
        const request = JSON.parse(input) as WireModelRequest;
        const maxOutputTokens = positiveInteger(
          process.env.RENOA_PI_MAX_OUTPUT_TOKENS,
          "RENOA_PI_MAX_OUTPUT_TOKENS",
        );
        return { ok: true, response: await invokeModel(request, runtime, maxOutputTokens) };
      }
    }
  } catch (error) {
    return failure(error);
  } finally {
    runtime.close();
  }
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

function requiredAction(value: string | undefined): "describe" | "invoke" {
  if (value === "describe" || value === "invoke") {
    return value;
  }
  throw new Error("RENOA_PI_ACTION must be describe or invoke");
}

function positiveInteger(value: string | undefined, name: string): number {
  const parsed = Number(value);
  if (!Number.isSafeInteger(parsed) || parsed <= 0) {
    throw new Error(`${name} must be a positive safe integer`);
  }
  return parsed;
}

function failure(error: unknown): BridgeResult {
  return {
    ok: false,
    error: error instanceof Error ? error.message : String(error),
    ...(error instanceof ModelInvocationError && error.kind !== undefined
      ? { error_kind: error.kind }
      : {}),
  };
}

async function readStdin(): Promise<string> {
  process.stdin.setEncoding("utf8");
  let input = "";
  for await (const chunk of process.stdin) {
    input += chunk;
  }
  return input;
}

main()
  .catch(failure)
  .then((result) => {
    process.stdout.write(`${JSON.stringify(result)}\n`);
  });
