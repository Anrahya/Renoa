import { createServer, type IncomingMessage, type Server, type ServerResponse } from "node:http";
import { mkdtempSync, rmSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { setTimeout as delay } from "node:timers/promises";

import { SqliteCredentialStore, type Credential } from "../src/credentials.js";
import type { JsonValue } from "../src/contract.js";
import type { RetryClock } from "../src/retry.js";
import type { Api, Model } from "../src/upstream/types.js";
import { findCatalogModel } from "../src/catalog.js";

export interface QueuedResponse {
  readonly status?: number;
  readonly headers?: Readonly<Record<string, string>>;
  readonly body?: string;
  readonly sse?: readonly string[];
  readonly partialSse?: readonly string[];
  readonly reset?: boolean;
  readonly hang?: boolean;
}

export interface RecordedRequest {
  readonly method?: string;
  readonly url?: string;
  readonly headers: Record<string, string>;
  readonly body: string;
}

export interface FakeServer {
  readonly baseUrl: string;
  readonly origin: string;
  readonly requests: RecordedRequest[];
  enqueue(response: QueuedResponse): void;
  close(): Promise<void>;
}

export function startFakeServer(): Promise<FakeServer> {
  const queue: QueuedResponse[] = [];
  const requests: RecordedRequest[] = [];
  const server: Server = createServer((request, response) => {
    void handle(request, response, queue, requests);
  });
  return new Promise((resolve, reject) => {
    server.listen(0, "127.0.0.1", () => {
      const address = server.address();
      if (address === null || typeof address === "string") {
        reject(new Error("fake server has no TCP address"));
        return;
      }
      const origin = `http://127.0.0.1:${address.port}`;
      resolve({
        origin,
        baseUrl: `${origin}/v1`,
        requests,
        enqueue: (response) => {
          queue.push(response);
        },
        close: () =>
          new Promise((closeResolve, closeReject) => {
            const timer = setTimeout(() => {
              closeReject(new Error("fake HTTP server close exceeded 5s"));
            }, 5_000);
            server.closeAllConnections();
            server.close((error) => {
              clearTimeout(timer);
              if (error) {
                closeReject(error);
              } else {
                closeResolve();
              }
            });
          }),
      });
    });
  });
}

export function sseChat(chunks: readonly unknown[]): string[] {
  return [...chunks.map((chunk) => `data: ${JSON.stringify(chunk)}`), "data: [DONE]"];
}

export function chatChunk(
  delta: Record<string, unknown>,
  extra: Record<string, unknown> = {},
): Record<string, unknown> {
  return {
    id: "chatcmpl-1",
    object: "chat.completion.chunk",
    created: 1,
    model: "grok-4.6",
    choices: [{ index: 0, delta, finish_reason: extra.finish_reason ?? null }],
    ...omit(extra, "finish_reason"),
  };
}

export function successfulChat(text = "Hello from xAI"): string[] {
  return sseChat([
    chatChunk({ role: "assistant", content: text }),
    chatChunk(
      {},
      {
        finish_reason: "stop",
        usage: {
          prompt_tokens: 12,
          completion_tokens: 4,
          total_tokens: 16,
          prompt_tokens_details: { cached_tokens: 2, cache_write_tokens: 1 },
        },
      },
    ),
  ]);
}

export function reasoningChat(text = "visible", thinking = "plan"): string[] {
  return sseChat([
    chatChunk({ reasoning_content: thinking }),
    chatChunk({ content: text }),
    chatChunk({}, { finish_reason: "stop", usage: { prompt_tokens: 8, completion_tokens: 6, total_tokens: 14 } }),
  ]);
}

export function toolChat(): string[] {
  return sseChat([
    chatChunk({
      tool_calls: [{ index: 0, id: "call_stable", type: "function", function: { name: "lookup", arguments: "" } }],
    }),
    chatChunk({
      tool_calls: [{ index: 0, function: { arguments: '{"q":' } }],
    }),
    chatChunk({
      tool_calls: [{ index: 0, function: { arguments: '"city"}' } }],
    }),
    chatChunk({}, { finish_reason: "tool_calls", usage: { prompt_tokens: 5, completion_tokens: 8, total_tokens: 13 } }),
  ]);
}

export function anthropicSse(text = "Hello from OpenCode"): string[] {
  return [
    event("message_start", {
      type: "message_start",
      message: {
        id: "msg_1",
        type: "message",
        role: "assistant",
        content: [],
        model: "minimax-m3",
        stop_reason: null,
        stop_sequence: null,
        usage: { input_tokens: 9, output_tokens: 0, cache_read_input_tokens: 1, cache_creation_input_tokens: 0 },
      },
    }),
    event("content_block_start", {
      type: "content_block_start",
      index: 0,
      content_block: { type: "text", text: "" },
    }),
    event("content_block_delta", {
      type: "content_block_delta",
      index: 0,
      delta: { type: "text_delta", text },
    }),
    event("content_block_stop", { type: "content_block_stop", index: 0 }),
    event("message_delta", {
      type: "message_delta",
      delta: { stop_reason: "end_turn", stop_sequence: null },
      usage: { output_tokens: 3 },
    }),
    event("message_stop", { type: "message_stop" }),
  ];
}

export function responsesSse(text = "Hello from Responses"): string[] {
  return [
    event("response.created", {
      type: "response.created",
      response: { id: "resp_1", model: "grok-4.5", status: "in_progress", output: [] },
    }),
    event("response.output_item.added", {
      type: "response.output_item.added",
      output_index: 0,
      item: { type: "message", id: "msg_1", role: "assistant", content: [], status: "in_progress" },
    }),
    event("response.output_text.delta", {
      type: "response.output_text.delta",
      output_index: 0,
      content_index: 0,
      delta: text,
    }),
    event("response.output_item.done", {
      type: "response.output_item.done",
      output_index: 0,
      item: {
        type: "message",
        id: "msg_1",
        role: "assistant",
        status: "completed",
        content: [{ type: "output_text", text }],
      },
    }),
    event("response.completed", {
      type: "response.completed",
      response: {
        id: "resp_1",
        model: "grok-4.5",
        status: "completed",
        output: [
          {
            type: "message",
            id: "msg_1",
            role: "assistant",
            status: "completed",
            content: [{ type: "output_text", text }],
          },
        ],
        usage: {
          input_tokens: 11,
          output_tokens: 5,
          total_tokens: 16,
          input_tokens_details: { cached_tokens: 3 },
        },
      },
    }),
  ];
}

export function jsonError(status: number, body: unknown): QueuedResponse {
  return {
    status,
    headers: { "content-type": "application/json", "x-request-id": "req_test_1" },
    body: JSON.stringify(body),
  };
}

export class ManualClock implements RetryClock {
  nowMs = 1_000;
  delays: number[] = [];
  pending: { resolve: () => void; reject: (error: Error) => void; signal: AbortSignal } | undefined;

  now(): number {
    return this.nowMs;
  }

  sleep(ms: number, signal: AbortSignal): Promise<void> {
    this.delays.push(ms);
    return new Promise((resolve, reject) => {
      if (signal.aborted) {
        reject(abortError());
        return;
      }
      const onAbort = () => {
        this.pending = undefined;
        reject(abortError());
      };
      signal.addEventListener("abort", onAbort, { once: true });
      this.pending = {
        resolve: () => {
          signal.removeEventListener("abort", onAbort);
          this.nowMs += ms;
          this.pending = undefined;
          resolve();
        },
        reject,
        signal,
      };
    });
  }

  release(): void {
    this.pending?.resolve();
  }
}

export function tempDir(): { path: string; close(): void } {
  const path = mkdtempSync(join(tmpdir(), "renoa-model-provider-"));
  return {
    path,
    close: () => rmSync(path, { recursive: true, force: true }),
  };
}

export function createStore(directory: string, credential: Credential, provider = "xai"): SqliteCredentialStore {
  const store = new SqliteCredentialStore(join(directory, "credentials.sqlite"), { busyTimeoutMs: 1_000 });
  store.write(provider, credential);
  return store;
}

export function oauthCredential(expires = Date.now() + 60 * 60 * 1000): Extract<Credential, { type: "oauth" }> {
  return {
    type: "oauth",
    access: "access-token-old",
    refresh: "refresh-token-old",
    expires,
  };
}

export function loopbackModel(
  provider: "xai" | "opencode-go",
  modelId: string,
  baseUrl: string,
): Model<Api> {
  const found = findCatalogModel(provider, modelId);
  if (found === undefined) {
    throw new Error(`missing catalog model ${provider}/${modelId}`);
  }
  return { ...found.model, baseUrl } as Model<Api>;
}

export function userRequest(text = "Hi"): {
  system_prompt: string;
  messages: { role: "user"; content: { type: "text"; text: string }[] }[];
  tools: { name: string; description: string; input_schema: JsonValue }[];
} {
  return {
    system_prompt: "You are a test model.",
    messages: [{ role: "user", content: [{ type: "text", text }] }],
    tools: [
      {
        name: "lookup",
        description: "Look something up",
        input_schema: { type: "object", properties: { q: { type: "string" } } },
      },
    ],
  };
}

function event(name: string, data: unknown): string {
  return `event: ${name}\ndata: ${JSON.stringify(data)}`;
}

function omit(value: Record<string, unknown>, key: string): Record<string, unknown> {
  const next = { ...value };
  delete next[key];
  return next;
}

function abortError(): Error {
  const error = new Error("The operation was aborted");
  error.name = "AbortError";
  return error;
}

async function handle(
  request: IncomingMessage,
  response: ServerResponse,
  queue: QueuedResponse[],
  requests: RecordedRequest[],
): Promise<void> {
  const chunks: Buffer[] = [];
  for await (const chunk of request) {
    chunks.push(Buffer.from(chunk));
  }
  requests.push({
    ...(request.method === undefined ? {} : { method: request.method }),
    ...(request.url === undefined ? {} : { url: request.url }),
    headers: Object.fromEntries(
      Object.entries(request.headers).map(([name, value]) => [
        name,
        Array.isArray(value) ? value.join(",") : (value ?? ""),
      ]),
    ),
    body: Buffer.concat(chunks).toString("utf8"),
  });
  const next = queue.shift();
  if (next === undefined) {
    response.writeHead(500, { "content-type": "application/json" });
    response.end(JSON.stringify({ error: { message: "no queued response" } }));
    return;
  }
  if (next.reset === true) {
    request.socket.destroy();
    return;
  }
  if (next.partialSse !== undefined) {
    response.writeHead(200, { "content-type": "text/event-stream" });
    for (const line of next.partialSse) {
      response.write(`${line}\n\n`);
    }
    await delay(100);
    request.socket.destroy();
    return;
  }
  if (next.hang === true) {
    const abort = new AbortController();
    const hung = (async () => {
      try {
        await delay(30_000, undefined, { signal: abort.signal });
      } catch {
        return "aborted" as const;
      }
      request.socket.destroy();
      throw new Error("fake HTTP hang was not shut down within 30s");
    })();
    try {
      await Promise.race([
        hung,
        new Promise<"closed">((resolve) => {
          if (request.socket.destroyed) {
            resolve("closed");
            return;
          }
          request.socket.once("close", () => resolve("closed"));
        }),
      ]);
    } finally {
      abort.abort();
      await hung.catch(() => undefined);
    }
    return;
  }
  const headers = { "content-type": next.sse === undefined ? "application/json" : "text/event-stream", ...next.headers };
  response.writeHead(next.status ?? 200, headers);
  if (next.sse !== undefined) {
    for (const line of next.sse) {
      response.write(`${line}\n\n`);
    }
    response.end();
    return;
  }
  response.end(next.body ?? "");
}
