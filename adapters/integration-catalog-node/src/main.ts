import { execute } from "./client.js";
import type { AdapterRecord } from "./contract.js";
import { CatalogProblem, failure, safeDiagnostic } from "./errors.js";
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
      const baseUrl = process.env.RENOA_INTEGRATIONS_BASE_URL;
      record = await execute(request, {
        ...(baseUrl === undefined ? {} : { baseUrl }),
        signal: cancellation.signal,
      });
    } catch (error) {
      record = {
        wire_version: WIRE_VERSION,
        event: "failed",
        failure: failure(error),
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
      throw new CatalogProblem(
        "resource_limit",
        `integration catalog request exceeds ${MAX_STDIN_BYTES} bytes`,
        { code: "stdin_limit" },
      );
    }
    chunks.push(buffer);
  }
  if (bytes === 0) {
    throw new CatalogProblem("invalid_request", "integration catalog request is empty", {
      code: "empty_request",
    });
  }
  try {
    return JSON.parse(Buffer.concat(chunks, bytes).toString("utf8")) as unknown;
  } catch (error) {
    throw new CatalogProblem(
      "invalid_request",
      "integration catalog request is not valid JSON",
      { code: "invalid_json", cause: error },
    );
  }
}

function writeRecord(record: AdapterRecord): Promise<void> {
  const encoded = `${JSON.stringify(record)}\n`;
  if (Buffer.byteLength(encoded, "utf8") > MAX_RECORD_BYTES) {
    return Promise.reject(
      new Error(`integration catalog record exceeds ${MAX_RECORD_BYTES} bytes`),
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
