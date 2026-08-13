import { loadModelConfig } from "./config.js";
import { invokeModel, type WireModelRequest } from "./model-bridge.js";
import { loadModelRuntime } from "./model-runtime.js";

interface BridgeResult {
  readonly ok: boolean;
  readonly response?: unknown;
  readonly error?: string;
}

async function main(): Promise<BridgeResult> {
  const input = await readStdin();
  const request = JSON.parse(input) as WireModelRequest;
  const runtime = await loadModelRuntime(loadModelConfig(process.env));
  try {
    return { ok: true, response: await invokeModel(request, runtime) };
  } finally {
    runtime.close();
  }
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
  .catch((error: unknown): BridgeResult => ({
    ok: false,
    error: error instanceof Error ? error.message : String(error),
  }))
  .then((result) => {
    process.stdout.write(`${JSON.stringify(result)}\n`);
  });
