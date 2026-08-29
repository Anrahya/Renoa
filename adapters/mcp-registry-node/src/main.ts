import { execute } from "./client.js";
import type { AdapterRecord } from "./contract.js";
import { RegistryProblem, safeDiagnostic, toWireFailure } from "./errors.js";
import { MAX_RECORD_BYTES, MAX_STDIN_BYTES, WIRE_VERSION } from "./limits.js";
import { parseRequest } from "./wire.js";

async function main(): Promise<void> {
  const cancellation = new AbortController();
  const cancel = () => cancellation.abort();
  process.once("SIGINT", cancel);
  process.once("SIGTERM", cancel);
  try {
    let record: AdapterRecord;
    try {
      const request = parseRequest(await readRequest());
      const baseUrl = process.env.RENOA_MCP_REGISTRY_BASE_URL;
      record = await execute(request, {
        ...(baseUrl === undefined ? {} : { baseUrl }),
        signal: cancellation.signal,
      });
    } catch (error) {
      record = {
        wire_version: WIRE_VERSION,
        event: "failed",
        failure: toWireFailure(error),
      };
    }
    await writeRecord(record);
  } finally {
    process.removeListener("SIGINT", cancel);
    process.removeListener("SIGTERM", cancel);
  }
}

async function readRequest(): Promise<unknown> {
  const chunks: Buffer[] = [];
  let bytes = 0;
  for await (const chunk of process.stdin) {
    const buffer = Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk);
    bytes += buffer.byteLength;
    if (bytes > MAX_STDIN_BYTES) {
      process.stdin.destroy();
      throw new RegistryProblem(
        "resource_limit",
        `MCP Registry adapter request exceeds ${MAX_STDIN_BYTES} bytes.`,
        { code: "stdin_limit" },
      );
    }
    chunks.push(buffer);
  }
  if (bytes === 0) {
    throw new RegistryProblem(
      "invalid_request",
      "MCP Registry adapter request is empty.",
      { code: "empty_request" },
    );
  }
  try {
    return JSON.parse(Buffer.concat(chunks, bytes).toString("utf8")) as unknown;
  } catch (error) {
    throw new RegistryProblem(
      "invalid_request",
      "MCP Registry adapter request is not valid JSON.",
      { code: "invalid_json", cause: error },
    );
  }
}

function writeRecord(record: AdapterRecord): Promise<void> {
  const encoded = `${JSON.stringify(record)}\n`;
  if (Buffer.byteLength(encoded, "utf8") > MAX_RECORD_BYTES) {
    return Promise.reject(
      new RegistryProblem(
        "resource_limit",
        `MCP Registry adapter record exceeds ${MAX_RECORD_BYTES} bytes.`,
        { code: "record_limit" },
      ),
    );
  }
  return new Promise((resolve, reject) => {
    process.stdout.write(encoded, (error) => {
      if (error === null || error === undefined) {
        resolve();
      } else {
        reject(error);
      }
    });
  });
}

main().catch((error: unknown) => {
  process.stderr.write(`${safeDiagnostic(error)}\n`);
  process.exitCode = 1;
});
