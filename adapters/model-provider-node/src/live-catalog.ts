import { randomUUID } from "node:crypto";
import { chmod, lstat, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { dirname } from "node:path";

import type { CatalogEntry } from "./catalog.js";
import { validateModelSpec } from "./catalog.js";
import type { ProviderId } from "./contract.js";
import { getSupportedThinkingLevels } from "@earendil-works/pi-ai";
import type { Api, Model } from "@earendil-works/pi-ai";

const CACHE_VERSION = 1;
const MAX_CACHE_BYTES = 4 * 1024 * 1024;

/**
 * A provider whose catalog Renoa refreshes from the provider's own model list.
 * The id rule and the model count bound what a response or a cache may hold.
 */
export interface LiveCatalogSource {
  readonly provider: ProviderId;
  readonly maxModels: number;
  isValidId(value: unknown): value is string;
}

/**
 * The last complete live result, kept beside the credential database so a
 * learned model stays bound, and stays available while the provider's list
 * cannot be reached.
 */
export interface LiveCatalogCache {
  readonly version: typeof CACHE_VERSION;
  readonly models_dev_etag?: string;
  readonly available_model_ids: readonly string[];
  readonly dynamic_models: readonly Model<Api>[];
}

export async function readLiveCatalogCache(
  path: string,
  source: LiveCatalogSource,
  pinned: readonly CatalogEntry[],
): Promise<LiveCatalogCache | undefined> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.size > MAX_CACHE_BYTES) {
      return undefined;
    }
    const parsed = JSON.parse(await readFile(path, "utf8")) as unknown;
    if (!isRecord(parsed) || parsed.version !== CACHE_VERSION) {
      return undefined;
    }
    const ids = parseCachedIds(parsed.available_model_ids, source);
    const dynamic = parseCachedModels(parsed.dynamic_models, source, pinned);
    const etag = parsed.models_dev_etag;
    if (etag !== undefined && (typeof etag !== "string" || etag.length > 1_024)) {
      return undefined;
    }
    return {
      version: CACHE_VERSION,
      ...(typeof etag === "string" ? { models_dev_etag: etag } : {}),
      available_model_ids: ids,
      dynamic_models: dynamic,
    };
  } catch {
    return undefined;
  }
}

export async function writeLiveCatalogCache(
  path: string,
  cache: Omit<LiveCatalogCache, "version">,
): Promise<void> {
  await mkdir(dirname(path), { recursive: true, mode: 0o700 });
  const temporary = `${path}.${process.pid}.${randomUUID()}.tmp`;
  let handle;
  try {
    handle = await open(temporary, "wx", 0o600);
    await handle.writeFile(`${JSON.stringify({ version: CACHE_VERSION, ...cache })}\n`, "utf8");
    await handle.sync();
    await handle.close();
    handle = undefined;
    await rename(temporary, path);
    await chmod(path, 0o600);
  } finally {
    if (handle !== undefined) {
      await handle.close().catch(() => undefined);
    }
    await unlink(temporary).catch(() => undefined);
  }
}

/**
 * The advertised catalog: every pinned model, and every learned model the
 * provider lists as available. A pinned entry wins over a learned one with the
 * same id.
 */
export function selectAvailableModels(
  pinned: readonly CatalogEntry[],
  dynamic: readonly Model<Api>[],
  availableIds: readonly string[],
): CatalogEntry[] {
  const available = new Set(availableIds);
  const byId = new Map(pinned.map((entry) => [entry.id, entry]));
  for (const model of dynamic) {
    if (available.has(model.id) && !byId.has(model.id)) {
      byId.set(model.id, toEntry(model));
    }
  }
  return [...byId.values()]
    .sort((left, right) => left.name.localeCompare(right.name) || left.id.localeCompare(right.id));
}

/**
 * Learned bindings stay stable: a model already cached keeps its binding, and
 * only a model seen for the first time is added.
 */
export function mergeLearnedModels(
  cached: readonly Model<Api>[],
  learned: readonly Model<Api>[],
): Model<Api>[] {
  const merged = [...cached];
  const known = new Set(cached.map((model) => model.id));
  for (const model of learned) {
    if (!known.has(model.id)) {
      known.add(model.id);
      merged.push(model);
    }
  }
  return merged;
}

export async function readBoundedJson(response: Response, maximumBytes: number): Promise<unknown> {
  const length = response.headers.get("content-length");
  if (length !== null && (!/^\d+$/u.test(length) || Number(length) > maximumBytes)) {
    throw new Error("catalog response exceeds its byte limit");
  }
  if (response.body === null) {
    throw new Error("catalog response has no body");
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let bytes = 0;
  for (;;) {
    const { done, value } = await reader.read();
    if (done) {
      break;
    }
    bytes += value.byteLength;
    if (bytes > maximumBytes) {
      await reader.cancel();
      throw new Error("catalog response exceeds its byte limit");
    }
    chunks.push(value);
  }
  const buffer = Buffer.concat(chunks.map((chunk) => Buffer.from(chunk)), bytes);
  return JSON.parse(new TextDecoder("utf-8", { fatal: true }).decode(buffer)) as unknown;
}

export function requestSignal(
  parent: AbortSignal | undefined,
  timeoutMs: number,
  label: string,
): AbortSignal {
  if (!Number.isSafeInteger(timeoutMs) || timeoutMs <= 0) {
    throw new Error(`${label} catalog timeout must be a positive integer`);
  }
  const timeout = AbortSignal.timeout(timeoutMs);
  return parent === undefined ? timeout : AbortSignal.any([parent, timeout]);
}

function toEntry(model: Model<Api>): CatalogEntry {
  return {
    id: model.id,
    name: model.name,
    reasoning_levels: getSupportedThinkingLevels(model),
    model,
  };
}

function parseCachedIds(value: unknown, source: LiveCatalogSource): string[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > source.maxModels) {
    throw new Error("invalid cached model ids");
  }
  const unique = new Set<string>();
  for (const id of value) {
    if (!source.isValidId(id) || !unique.add(id)) {
      throw new Error("invalid cached model id");
    }
  }
  return [...unique].sort();
}

function parseCachedModels(
  value: unknown,
  source: LiveCatalogSource,
  pinned: readonly CatalogEntry[],
): Model<Api>[] {
  if (!Array.isArray(value) || value.length > source.maxModels) {
    throw new Error("invalid cached dynamic models");
  }
  const pinnedIds = new Set(pinned.map((entry) => entry.id));
  const unique = new Set<string>();
  const models: Model<Api>[] = [];
  for (const raw of value) {
    const id = isRecord(raw) ? raw.id : undefined;
    if (!source.isValidId(id) || !unique.add(id)) {
      throw new Error("invalid cached dynamic model");
    }
    // A cache written before an upgrade may hold a model the pinned catalog
    // now carries; the pinned entry wins and the cached one is skipped.
    if (!pinnedIds.has(id)) {
      models.push(validateModelSpec(raw, source.provider, id));
    }
  }
  return models;
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
