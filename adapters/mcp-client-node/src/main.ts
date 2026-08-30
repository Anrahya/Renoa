import { executeAdapterRequest } from "./client.js";
import type { AdapterRecord } from "./contract.js";
import { AdapterProblem, safeDiagnostic, toWireFailure } from "./errors.js";
import { MAX_RECORD_BYTES, MAX_STDIN_BYTES, WIRE_VERSION } from "./limits.js";
import { isOAuthRequest, oauthSecrets } from "./oauth.js";
import { parseAdapterRequest } from "./wire.js";

async function main(): Promise<void> {
  const cancellation = new AbortController();
  const cancel = () => cancellation.abort();
  process.once("SIGINT", cancel);
  process.once("SIGTERM", cancel);

  let cleanup: (() => Promise<void>) | undefined;
  let exactSecrets: string[] = [];
  let terminalWritten = false;
  try {
    let terminal: AdapterRecord;
    try {
      const request = parseAdapterRequest(await readRequest());
      if (isOAuthRequest(request)) {
        if (request.action === "oauth_begin") {
          exactSecrets = [request.csrf_state];
          if (request.oauth_state !== undefined) {
            exactSecrets.push(...oauthSecrets(request.oauth_state));
          }
        } else {
          exactSecrets = oauthSecrets(request.oauth_state);
          if (request.action === "oauth_exchange") {
            exactSecrets.push(request.authorization_code);
          }
        }
      } else if (request.credential !== undefined) {
        exactSecrets = [request.credential.secret];
      }
      terminal = await executeAdapterRequest(request, {
        signal: cancellation.signal,
        dispatchStarted: () =>
          writeRecord({
            wire_version: WIRE_VERSION,
            event: "dispatch_started",
          }),
        registerCleanup(next) {
          if (cleanup !== undefined) {
            throw new AdapterProblem(
              "internal",
              "MCP adapter registered more than one client cleanup.",
              { code: "duplicate_cleanup" },
            );
          }
          cleanup = next;
        },
      });
    } catch (error) {
      terminal = {
        wire_version: WIRE_VERSION,
        event: "failed",
        failure: toWireFailure(error),
      };
    }

    await writeRecord(terminal);
    terminalWritten = true;
  } finally {
    process.removeListener("SIGINT", cancel);
    process.removeListener("SIGTERM", cancel);
    if (cleanup !== undefined) {
      try {
        await cleanup();
      } catch (error) {
        process.stderr.write(
          `MCP cleanup failed after terminal: ${safeDiagnostic(error, exactSecrets)}\n`,
        );
        if (terminalWritten) {
          process.exitCode = 1;
        }
      }
    }
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
      throw new AdapterProblem(
        "resource_limit",
        `MCP adapter request exceeds ${MAX_STDIN_BYTES} bytes.`,
        { code: "stdin_limit" },
      );
    }
    chunks.push(buffer);
  }
  if (bytes === 0) {
    throw new AdapterProblem(
      "invalid_request",
      "MCP adapter request is empty.",
      {
        code: "empty_request",
      },
    );
  }
  try {
    return JSON.parse(Buffer.concat(chunks, bytes).toString("utf8")) as unknown;
  } catch (error) {
    throw new AdapterProblem(
      "invalid_request",
      "MCP adapter request is not valid JSON.",
      {
        code: "invalid_json",
        cause: error,
      },
    );
  }
}

function writeRecord(record: AdapterRecord): Promise<void> {
  const encoded = `${JSON.stringify(record)}\n`;
  if (Buffer.byteLength(encoded, "utf8") > MAX_RECORD_BYTES) {
    return Promise.reject(
      new AdapterProblem(
        "resource_limit",
        `MCP adapter record exceeds ${MAX_RECORD_BYTES} bytes.`,
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
