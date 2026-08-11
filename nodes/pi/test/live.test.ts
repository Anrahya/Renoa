import assert from "node:assert/strict";
import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import { createServer, type Server, type ServerResponse } from "node:http";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";
import { DatabaseSync } from "node:sqlite";
import { test } from "node:test";

import type { StreamFn } from "@earendil-works/pi-agent-core";
import { streamSimple as streamOpenAI } from "@earendil-works/pi-ai/api/openai-completions";
import type { Context, Model, SimpleStreamOptions } from "@earendil-works/pi-ai";

import { PiHarness } from "../src/harness.js";
import { PiNode } from "../src/node.js";
import { RCP_VERSION, type DeviceCredentials } from "../src/protocol.js";

test("a real Pi edit turn survives a lost RCP acknowledgement", { timeout: 20_000 }, async () => {
  const directory = await mkdtemp(join(tmpdir(), "renoa-pi-live-"));
  await writeFile(join(directory, "proof.txt"), "RCP read capability works.\n");
  const fixture = await startFixture(directory);
  const provider = await startOpenAI();
  const nodeAbort = new AbortController();
  const model: Model<"openai-completions"> = {
    id: "test-model",
    name: "Test model",
    api: "openai-completions",
    provider: "openai-compatible",
    baseUrl: provider.baseUrl,
    reasoning: false,
    input: ["text"],
    cost: { input: 0, output: 0, cacheRead: 0, cacheWrite: 0 },
    contextWindow: 16_384,
    maxTokens: 1_024,
  };
  const streamFn: StreamFn = (requested, context, options) =>
    streamOpenAI(requested as Model<"openai-completions">, context as Context, {
      ...options,
      apiKey: "test-key",
      maxRetries: 0,
    } satisfies SimpleStreamOptions);
  const statePath = join(directory, "node.sqlite");
  const node = new PiNode({
    endpoint: fixture.lossyEndpoint,
    credentials: fixture.nodeCredentials,
    statePath,
    harness: new PiHarness({
      instructions: "Answer from the Pi SDK. Be concise.",
      model,
      streamFn,
      target: "workspace:pi-test",
      workspace: { root: directory, access: "read_write" },
    }),
  });
  const nodeTask = node.run(nodeAbort.signal);

  try {
    const surface = await Surface.connect(fixture.endpoint, fixture.surfaceCredentials);
    try {
      surface.send({
        type: "attach",
        request_id: 1,
        task_id: fixture.taskId,
        after_sequence: null,
      });
      assert.deepEqual(await surface.next(), {
        type: "attached",
        request_id: 1,
        task_id: fixture.taskId,
        through_sequence: null,
      });

      const commandId = "00000000-0000-0000-0000-000000000010";
      const events = await submitWhenOnline(surface, fixture.taskId, commandId);
      while (!events.some(isTerminalTaskEvent)) {
        const message = await surface.next();
        if (isTaskEvent(message)) {
          events.push(message);
        }
      }

      const activity = events
        .filter(isTaskEvent)
        .map((message) => executionKind(message))
        .filter((kind) => kind !== null);
      assert.deepEqual(activity, [
        { type: "execution_started" },
        { type: "turn_started" },
        { type: "assistant_message", text: "" },
        {
          type: "tool_started",
          call_id: "call-read-live",
          name: "read",
          arguments: { path: "proof.txt" },
        },
        {
          type: "tool_finished",
          call_id: "call-read-live",
          output: "RCP read capability works.\n",
          is_error: false,
        },
        { type: "turn_started" },
        { type: "assistant_message", text: "" },
        {
          type: "tool_started",
          call_id: "call-edit-live",
          name: "edit",
          arguments: {
            path: "proof.txt",
            edits: [{ oldText: "works", newText: "survives" }],
          },
        },
        {
          type: "tool_finished",
          call_id: "call-edit-live",
          output: "Successfully replaced 1 block(s) in proof.txt.",
          is_error: false,
        },
        { type: "turn_started" },
        { type: "assistant_message", text: "Pi edited through RCP." },
        { type: "execution_terminated", terminal: { status: "completed" } },
      ]);
      assert.equal(await readFile(join(directory, "proof.txt"), "utf8"), "RCP read capability survives.\n");
      await waitForPublication(statePath, commandId, 11);
      assert.match(JSON.stringify(provider.requests), /Answer from the Pi SDK/);
      assert.match(JSON.stringify(provider.requests), /read proof.txt, replace works with survives, and report/);
      assert.match(JSON.stringify(provider.requests), /RCP read capability works/);
    } finally {
      surface.close();
    }
  } finally {
    nodeAbort.abort();
    await nodeTask;
    await provider.stop();
    await fixture.stop();
    await rm(directory, { force: true, recursive: true });
  }
});

async function submitWhenOnline(
  surface: Surface,
  taskId: string,
  commandId: string,
): Promise<unknown[]> {
  const observed: unknown[] = [];
  for (let requestId = 2; requestId < 20; requestId++) {
    surface.send({
      type: "submit",
      request_id: requestId,
      task_id: taskId,
      command_id: commandId,
      input: {
        type: "text",
        text: "read proof.txt, replace works with survives, and report",
      },
    });
    for (;;) {
      const message = await surface.next();
      if (isTaskEvent(message)) {
        observed.push(message);
        continue;
      }
      const record = asRecord(message);
      if (record.type === "command_accepted" && record.request_id === requestId) {
        return observed;
      }
      if (
        record.type === "error" &&
        record.request_id === requestId &&
        record.code === "node_offline"
      ) {
        await new Promise((resolveDelay) => setTimeout(resolveDelay, 50));
        break;
      }
      throw new Error(`unexpected submit response: ${JSON.stringify(message)}`);
    }
  }
  throw new Error("Pi node did not connect to the coordinator");
}

function isTaskEvent(value: unknown): value is Record<string, unknown> {
  return asRecord(value).type === "task_event";
}

function isTerminalTaskEvent(value: unknown): boolean {
  return executionKind(value)?.type === "execution_terminated";
}

function executionKind(value: unknown): Record<string, unknown> | null {
  const message = asRecord(value);
  if (message.type !== "task_event") return null;
  const event = asRecord(message.event);
  const taskKind = asRecord(event.kind);
  if (taskKind.type !== "execution_event") return null;
  return asRecord(asRecord(taskKind.event).kind);
}

class Surface {
  readonly #socket: WebSocket;
  readonly #messages: unknown[] = [];
  readonly #waiters: Array<{
    readonly resolve: (message: unknown) => void;
    readonly reject: (error: Error) => void;
  }> = [];

  private constructor(socket: WebSocket) {
    this.#socket = socket;
    socket.addEventListener("message", (event) => {
      if (typeof event.data !== "string") {
        this.#fail(new Error("surface received a non-text message"));
        return;
      }
      const message: unknown = JSON.parse(event.data);
      const waiter = this.#waiters.shift();
      if (waiter === undefined) {
        this.#messages.push(message);
      } else {
        waiter.resolve(message);
      }
    });
    socket.addEventListener("close", () => this.#fail(new Error("surface connection closed")));
  }

  static async connect(endpoint: string, credentials: DeviceCredentials): Promise<Surface> {
    const socket = new WebSocket(endpoint);
    await new Promise<void>((resolveOpen, reject) => {
      socket.addEventListener("open", () => resolveOpen(), { once: true });
      socket.addEventListener("error", () => reject(new Error("surface connection failed")), {
        once: true,
      });
    });
    const surface = new Surface(socket);
    surface.send({ type: "authenticate", version: RCP_VERSION, credentials });
    assert.deepEqual(await surface.next(), { type: "authenticated", version: RCP_VERSION });
    return surface;
  }

  send(message: object): void {
    this.#socket.send(JSON.stringify(message));
  }

  next(): Promise<unknown> {
    const message = this.#messages.shift();
    if (message !== undefined) {
      return Promise.resolve(message);
    }
    return new Promise((resolveMessage, reject) => {
      this.#waiters.push({ resolve: resolveMessage, reject });
    });
  }

  close(): void {
    this.#socket.close();
  }

  #fail(error: Error): void {
    for (const waiter of this.#waiters.splice(0)) {
      waiter.reject(error);
    }
  }
}

interface Fixture {
  readonly endpoint: string;
  readonly lossyEndpoint: string;
  readonly nodeCredentials: DeviceCredentials;
  readonly surfaceCredentials: DeviceCredentials;
  readonly taskId: string;
  stop(): Promise<void>;
}

async function waitForPublication(
  statePath: string,
  commandId: string,
  throughSequence: number,
): Promise<void> {
  const database = new DatabaseSync(statePath, { readOnly: true });
  try {
    for (let attempt = 0; attempt < 100; attempt++) {
      const row = database
        .prepare(
          "SELECT admission_acked, published_through FROM executions WHERE command_id = ?",
        )
        .get(commandId) as
        | { readonly admission_acked: number; readonly published_through: number | null }
        | undefined;
      if (row?.admission_acked === 1 && row.published_through === throughSequence) {
        return;
      }
      await new Promise((resolveDelay) => setTimeout(resolveDelay, 25));
    }
  } finally {
    database.close();
  }
  throw new Error("Pi node did not recover its lost event acknowledgement");
}

async function startFixture(directory: string): Promise<Fixture> {
  const repository = resolve(import.meta.dirname, "../../..");
  const process = spawn(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "renoa-control",
      "--example",
      "pi_node_fixture",
      "--",
      join(directory, "control.sqlite"),
    ],
    { cwd: repository, stdio: ["ignore", "pipe", "inherit"] },
  );
  const description = (await firstLine(process)) as Omit<Fixture, "stop">;
  return {
    ...description,
    stop: () => stopProcess(process),
  };
}

async function firstLine(process: ChildProcess): Promise<unknown> {
  if (process.stdout === null) throw new Error("fixture stdout is unavailable");
  const lines = createInterface({ input: process.stdout });
  for await (const line of lines) {
    lines.close();
    return JSON.parse(line);
  }
  throw new Error("fixture exited before startup");
}

async function stopProcess(process: ChildProcess): Promise<void> {
  if (process.exitCode !== null || process.signalCode !== null) return;
  const exited = new Promise<void>((resolveExit) => process.once("exit", () => resolveExit()));
  process.kill("SIGTERM");
  await exited;
}

interface OpenAIServer {
  readonly baseUrl: string;
  readonly requests: unknown[];
  stop(): Promise<void>;
}

async function startOpenAI(): Promise<OpenAIServer> {
  const requests: unknown[] = [];
  const server = createServer(async (request, response) => {
    const chunks: Buffer[] = [];
    for await (const chunk of request) chunks.push(Buffer.from(chunk));
    requests.push(JSON.parse(Buffer.concat(chunks).toString("utf8")));
    response.writeHead(200, { "content-type": "text/event-stream" });
    if (requests.length === 1) {
      writeChunk(response, {
        role: "assistant",
        tool_calls: [
          {
            index: 0,
            id: "call-read-live",
            type: "function",
            function: { name: "read", arguments: '{"path":"proof.txt"}' },
          },
        ],
      });
      writeChunk(response, {}, "tool_calls");
    } else if (requests.length === 2) {
      writeChunk(response, {
        role: "assistant",
        tool_calls: [
          {
            index: 0,
            id: "call-edit-live",
            type: "function",
            function: {
              name: "edit",
              arguments:
                '{"path":"proof.txt","edits":[{"oldText":"works","newText":"survives"}]}',
            },
          },
        ],
      });
      writeChunk(response, {}, "tool_calls");
    } else {
      writeChunk(response, { role: "assistant", content: "Pi edited through RCP." });
      writeChunk(response, {}, "stop");
    }
    response.end("data: [DONE]\n\n");
  });
  await listen(server);
  const address = server.address();
  if (address === null || typeof address === "string") throw new Error("OpenAI fixture has no port");
  return {
    baseUrl: `http://127.0.0.1:${address.port}/v1`,
    requests,
    stop: () => closeServer(server),
  };
}

function writeChunk(
  response: ServerResponse,
  delta: object,
  finishReason: string | null = null,
): void {
  response.write(
    `data: ${JSON.stringify({
      id: "chatcmpl-live",
      object: "chat.completion.chunk",
      created: 1,
      model: "test-model",
      choices: [{ index: 0, delta, finish_reason: finishReason }],
    })}\n\n`,
  );
}

function listen(server: Server): Promise<void> {
  return new Promise((resolveListen, reject) => {
    server.once("error", reject);
    server.listen(0, "127.0.0.1", () => {
      server.removeListener("error", reject);
      resolveListen();
    });
  });
}

function closeServer(server: Server): Promise<void> {
  return new Promise((resolveClose, reject) => {
    server.close((error) => (error === undefined ? resolveClose() : reject(error)));
  });
}

function asRecord(value: unknown): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw new Error(`expected object, received ${JSON.stringify(value)}`);
  }
  return value as Record<string, unknown>;
}
