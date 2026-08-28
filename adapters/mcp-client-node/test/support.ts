import {
  createServer,
  type IncomingHttpHeaders,
  type IncomingMessage,
  type Server,
  type ServerResponse,
} from "node:http";
import { spawn, type ChildProcessWithoutNullStreams } from "node:child_process";
import { once } from "node:events";
import { fileURLToPath } from "node:url";
import type {
  AdapterRecord,
  AdapterRequest,
  JsonObject,
} from "../src/contract.js";
import { WIRE_VERSION } from "../src/limits.js";

export interface FixtureRequest {
  readonly url: string;
  readonly method: string;
  readonly headers: IncomingHttpHeaders;
  readonly rpc: {
    readonly jsonrpc: "2.0";
    readonly id?: string | number;
    readonly method: string;
    readonly params?: JsonObject;
  };
}

export interface FixtureResponse {
  readonly status?: number;
  readonly headers?: Readonly<Record<string, string>>;
  readonly transport?: "json" | "sse";
  readonly body: unknown;
  readonly destroySocket?: boolean;
  readonly keepOpenAfterBody?: boolean;
}

export class McpFixtureServer {
  readonly requests: FixtureRequest[] = [];
  readonly transportRequests: Array<{
    readonly url: string;
    readonly method: string;
    readonly headers: IncomingHttpHeaders;
  }> = [];
  readonly #server: Server;
  readonly #handler: (
    request: FixtureRequest,
  ) => FixtureResponse | Promise<FixtureResponse>;
  #url: URL | undefined;

  constructor(
    handler: (
      request: FixtureRequest,
    ) => FixtureResponse | Promise<FixtureResponse>,
  ) {
    this.#handler = handler;
    this.#server = createServer((request, response) => {
      void this.#respond(request, response).catch((error: unknown) => {
        if (!response.headersSent) {
          response.writeHead(500, { "content-type": "text/plain" });
        }
        response.end(error instanceof Error ? error.message : String(error));
      });
    });
  }

  get endpoint(): string {
    if (this.#url === undefined) {
      throw new Error("fixture server is not started");
    }
    return this.#url.href;
  }

  async start(): Promise<void> {
    this.#server.listen(0, "127.0.0.1");
    await once(this.#server, "listening");
    const address = this.#server.address();
    if (address === null || typeof address === "string") {
      throw new Error("fixture server did not bind a TCP address");
    }
    this.#url = new URL(`http://127.0.0.1:${address.port}/mcp`);
  }

  async close(): Promise<void> {
    this.#server.closeAllConnections();
    await new Promise<void>((resolve, reject) => {
      this.#server.close((error) =>
        error === undefined ? resolve() : reject(error),
      );
    });
  }

  async #respond(
    request: IncomingMessage,
    response: ServerResponse,
  ): Promise<void> {
    const transportRequest = {
      url: request.url ?? "",
      method: request.method ?? "",
      headers: request.headers,
    };
    this.transportRequests.push(transportRequest);
    if (transportRequest.method === "GET") {
      response.writeHead(405, { "content-type": "text/plain" });
      response.end("event stream unavailable");
      return;
    }
    const raw = await collectStream(request);
    const parsed = JSON.parse(raw) as unknown;
    if (!isRpcMessage(parsed)) {
      response.writeHead(400, { "content-type": "text/plain" });
      response.end("invalid JSON-RPC request");
      return;
    }
    const observed: FixtureRequest = {
      url: request.url ?? "",
      method: request.method ?? "",
      headers: request.headers,
      rpc: parsed,
    };
    this.requests.push(observed);
    const planned = await this.#handler(observed);
    if (planned.destroySocket === true) {
      request.socket.destroy();
      return;
    }

    const status = planned.status ?? 200;
    const transport = planned.transport ?? "json";
    const encoded = JSON.stringify(planned.body);
    const body =
      transport === "sse" ? `event: message\ndata: ${encoded}\n\n` : encoded;
    response.writeHead(status, {
      "content-type":
        transport === "sse" ? "text/event-stream" : "application/json",
      ...planned.headers,
    });
    if (planned.keepOpenAfterBody === true) {
      response.write(body);
    } else {
      response.end(body);
    }
  }
}

export function rpcResult(
  request: FixtureRequest,
  result: Readonly<Record<string, unknown>>,
): Readonly<Record<string, unknown>> {
  if (request.rpc.id === undefined) {
    throw new Error(`notification '${request.rpc.method}' cannot receive a result`);
  }
  return { jsonrpc: "2.0", id: request.rpc.id, result };
}

export function rpcError(
  request: FixtureRequest,
  code: number,
  message: string,
): Readonly<Record<string, unknown>> {
  if (request.rpc.id === undefined) {
    throw new Error(`notification '${request.rpc.method}' cannot receive an error`);
  }
  return { jsonrpc: "2.0", id: request.rpc.id, error: { code, message } };
}

export function discoverResult(
  request: FixtureRequest,
  supportedVersions: readonly string[] = ["2026-07-28"],
): FixtureResponse {
  return {
    body: rpcResult(request, {
      resultType: "complete",
      supportedVersions,
      capabilities: { tools: {} },
    }),
  };
}

export interface ProcessResult {
  readonly exitCode: number | null;
  readonly records: readonly AdapterRecord[];
  readonly stderr: string;
}

export interface RunAdapterOptions {
  readonly onSpawn?: (child: ChildProcessWithoutNullStreams) => void;
}

export const EMPTY_SCHEMA: JsonObject = { type: "object", properties: {} };

export const HEADER_SCHEMA: JsonObject = {
  type: "object",
  properties: {
    tenant: {
      type: "string",
      "x-mcp-header": "Tenant",
    },
  },
  required: ["tenant"],
};

export function discoverRequest(
  endpoint: string,
): Extract<AdapterRequest, { readonly action: "discover" }> {
  return { wire_version: WIRE_VERSION, action: "discover", endpoint };
}

export function callRequest(
  endpoint: string,
  protocolVersion = "2026-07-28",
): Extract<AdapterRequest, { readonly action: "call" }> {
  return {
    wire_version: WIRE_VERSION,
    action: "call",
    endpoint,
    protocol_version: protocolVersion,
    tool: {
      name: "write_note",
      input_schema: HEADER_SCHEMA,
    },
    arguments: { tenant: "renoa" },
  };
}

export async function runAdapter(
  request: AdapterRequest,
  options: RunAdapterOptions = {},
): Promise<ProcessResult> {
  const executable = fileURLToPath(new URL("../src/main.js", import.meta.url));
  const child = spawn(process.execPath, [executable], {
    stdio: ["pipe", "pipe", "pipe"],
  });
  options.onSpawn?.(child);
  const stdout = collectStream(child.stdout);
  const stderr = collectStream(child.stderr);
  child.stdin.end(JSON.stringify(request));

  let timer: NodeJS.Timeout | undefined;
  const timeout = new Promise<never>((_resolve, reject) => {
    timer = setTimeout(() => {
      child.kill("SIGKILL");
      reject(new Error("MCP adapter process did not exit within 5 seconds"));
    }, 5_000);
  });
  const closed = once(child, "close").then(([code]) => code as number | null);
  const exitCode = await Promise.race([closed, timeout]).finally(() => {
    if (timer !== undefined) {
      clearTimeout(timer);
    }
  });
  const [output, diagnostics] = await Promise.all([stdout, stderr]);
  const records = output
    .split("\n")
    .filter((line) => line.length > 0)
    .map((line) => JSON.parse(line) as AdapterRecord);
  return { exitCode, records, stderr: diagnostics };
}

async function collectStream(stream: NodeJS.ReadableStream): Promise<string> {
  const chunks: Buffer[] = [];
  for await (const chunk of stream) {
    chunks.push(Buffer.isBuffer(chunk) ? chunk : Buffer.from(chunk));
  }
  return Buffer.concat(chunks).toString("utf8");
}

function isRpcMessage(value: unknown): value is FixtureRequest["rpc"] {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    return false;
  }
  const record = value as Record<string, unknown>;
  return (
    record.jsonrpc === "2.0" &&
    (record.id === undefined ||
      typeof record.id === "string" ||
      typeof record.id === "number") &&
    typeof record.method === "string" &&
    (record.params === undefined ||
      (typeof record.params === "object" &&
        record.params !== null &&
        !Array.isArray(record.params)))
  );
}
