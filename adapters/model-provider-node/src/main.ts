import { loadBridgeConfig } from "./config.js";
import { runBridgeAction, type DescriptionResult } from "./bridge.js";
import type { WireStreamRecord } from "./contract.js";
import { parseWireModelRequest } from "./wire-request.js";
import { wireError } from "./stream.js";

async function main(): Promise<void> {
  const config = loadBridgeConfig(process.env);
  const cancellation = new AbortController();
  const cancel = () => cancellation.abort();
  process.once("SIGINT", cancel);
  process.once("SIGTERM", cancel);
  try {
    const request = config.action === "stream" ? await readRequest() : undefined;
    await runBridgeAction(config, request, writeRecord, cancellation.signal);
  } finally {
    process.removeListener("SIGINT", cancel);
    process.removeListener("SIGTERM", cancel);
  }
}

async function readRequest() {
  const input = await readStdin();
  let parsed: unknown;
  try {
    parsed = JSON.parse(input) as unknown;
  } catch {
    throw Object.assign(new Error("model stream request is not valid JSON"), {
      categoryHint: "invalid_request",
    });
  }
  return parseWireModelRequest(parsed);
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
  if (process.env.RENOA_MODEL_ACTION === "stream") {
    const record = wireError(error, {
      provider: process.env.RENOA_MODEL_PROVIDER === "opencode-go" ? "opencode-go" : "xai",
      model: process.env.RENOA_MODEL ?? "unknown",
    });
    process.stdout.write(`${JSON.stringify(record)}\n`);
  } else {
    process.stderr.write(`${error instanceof Error ? error.message : String(error)}\n`);
  }
  process.exitCode = 1;
});
