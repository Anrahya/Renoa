import assert from "node:assert/strict";
import { join } from "node:path";

import type { Credential } from "../src/credentials.js";
import type { WireStreamRecord } from "../src/contract.js";
import { ProviderFailure } from "../src/errors.js";
import { loadRuntime } from "../src/runtime.js";
import { streamModel } from "../src/stream.js";
import {
  ManualClock,
  createStore,
  loopbackModel,
  startFakeServer,
  tempDir,
  userRequest,
} from "./helpers.js";

export interface StreamOptions {
  directory: string;
  modelId: string;
  baseUrl: string;
  credential: Credential;
  provider?: "xai" | "opencode-go";
  reasoningLevel?: "low" | "medium" | "high" | "xhigh" | "max";
  clock?: ManualClock;
  signal?: AbortSignal;
  releases?: number;
  emit?: (record: WireStreamRecord) => void;
  fetch?: typeof fetch;
}

export async function runStream(options: StreamOptions): Promise<WireStreamRecord[]> {
  const records: WireStreamRecord[] = [];
  const store = createStore(options.directory, options.credential, options.provider ?? "xai");
  store.close();
  const model = loopbackModel(options.provider ?? "xai", options.modelId, options.baseUrl);
  const runtime = loadRuntime({
    provider: options.provider ?? "xai",
    modelId: options.modelId,
    authStorePath: join(options.directory, "credentials.sqlite"),
    modelSpec: model,
    allowLoopback: true,
    ...(options.reasoningLevel === undefined ? {} : { reasoningLevel: options.reasoningLevel }),
  });
  const abort = options.signal ?? new AbortController().signal;
  try {
    const work = streamModel({
      runtime,
      request: userRequest(),
      maxOutputTokens: 128,
      signal: abort,
      emit: async (record) => {
        records.push(record);
        options.emit?.(record);
      },
      ...(options.clock === undefined ? {} : { clock: options.clock }),
      ...(options.fetch === undefined ? {} : { fetch: options.fetch }),
      random: { jitter: () => 0 },
    });
    await releaseClock(options.clock, options.releases ?? 0, work);
    return records;
  } finally {
    runtime.close();
  }
}

export async function streamFailure(options: StreamOptions): Promise<ProviderFailure> {
  try {
    await runStream(options);
    throw new Error("expected provider failure");
  } catch (error) {
    if (error instanceof ProviderFailure) {
      return error;
    }
    throw error;
  }
}

export async function withOpenCode(
  modelId: string,
  setup: (server: Awaited<ReturnType<typeof startFakeServer>>) => string,
  verify: (
    records: WireStreamRecord[],
    server: Awaited<ReturnType<typeof startFakeServer>>,
  ) => void,
): Promise<void> {
  const server = await startFakeServer();
  const directory = tempDir();
  try {
    const baseUrl = setup(server);
    const records = await runStream({
      directory: directory.path,
      provider: "opencode-go",
      modelId,
      baseUrl,
      credential: { type: "api_key", key: "opencode-test-key" },
    });
    verify(records, server);
  } finally {
    await server.close();
    directory.close();
  }
}

export function deltas(
  records: readonly WireStreamRecord[],
  type: "text" | "reasoning" | "tool_call_arguments",
): string[] {
  return records.flatMap((record) => {
    if (record.event !== "content_delta") {
      return [];
    }
    if (type === "tool_call_arguments") {
      return record.delta.type === "tool_call_arguments" ? [record.delta.json_delta] : [];
    }
    return record.delta.type === type ? [record.delta.text] : [];
  });
}

export function completedRecord(
  records: readonly WireStreamRecord[],
): Extract<WireStreamRecord, { event: "completed" }> {
  const completed = records.find((record) => record.event === "completed");
  assert.equal(completed?.event, "completed");
  return completed as Extract<WireStreamRecord, { event: "completed" }>;
}

export async function waitFor(predicate: () => boolean): Promise<void> {
  for (let attempt = 0; attempt < 200; attempt += 1) {
    if (predicate()) {
      return;
    }
    await new Promise((resolve) => {
      setTimeout(resolve, 10);
    });
  }
  throw new Error("condition was not met");
}

async function releaseClock(
  clock: ManualClock | undefined,
  releases: number,
  work: Promise<void>,
): Promise<void> {
  if (clock === undefined || releases === 0) {
    await work;
    return;
  }
  let finished = false;
  const pumping = (async () => {
    for (let index = 0; index < releases && !finished; index += 1) {
      await waitFor(() => clock.pending !== undefined || finished);
      if (clock.pending !== undefined) {
        clock.release();
      }
    }
  })();
  try {
    await work;
  } finally {
    finished = true;
    clock.release();
    await pumping;
  }
}
