import { randomUUID } from "node:crypto";
import { chmod, lstat, mkdir, open, readFile, rename, unlink } from "node:fs/promises";
import { dirname } from "node:path";

import type { CatalogEntry } from "./catalog.js";
import { validateModelSpec } from "./catalog.js";
import type { Api, Model } from "./upstream/types.js";

const CACHE_VERSION = 1;
const MAX_CACHE_BYTES = 1024 * 1024;
const MODEL_ID = /^[A-Za-z0-9][A-Za-z0-9._:-]{0,127}$/u;
export const MAX_CATALOG_MODELS = 512;

export interface OpenCodeCatalogCache {
  readonly version: typeof CACHE_VERSION;
  readonly models_dev_etag?: string;
  readonly available_model_ids: readonly string[];
  readonly dynamic_models: readonly Model<Api>[];
}

export async function readOpenCodeCatalogCache(
  path: string,
  pinned: readonly CatalogEntry[],
): Promise<OpenCodeCatalogCache | undefined> {
  try {
    const metadata = await lstat(path);
    if (!metadata.isFile() || metadata.size > MAX_CACHE_BYTES) {
      return undefined;
    }
    const parsed = JSON.parse(await readFile(path, "utf8")) as unknown;
    if (!isRecord(parsed) || parsed.version !== CACHE_VERSION) {
      return undefined;
    }
    const ids = parseCachedIds(parsed.available_model_ids);
    const dynamic = parseCachedModels(parsed.dynamic_models, pinned);
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

export async function writeOpenCodeCatalogCache(
  path: string,
  cache: Omit<OpenCodeCatalogCache, "version">,
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

export function isValidOpenCodeModelId(value: unknown): value is string {
  return typeof value === "string" && MODEL_ID.test(value);
}

function parseCachedIds(value: unknown): string[] {
  if (!Array.isArray(value) || value.length === 0 || value.length > MAX_CATALOG_MODELS) {
    throw new Error("invalid cached model ids");
  }
  const unique = new Set<string>();
  for (const id of value) {
    if (!isValidOpenCodeModelId(id) || !unique.add(id)) {
      throw new Error("invalid cached model id");
    }
  }
  return [...unique].sort();
}

function parseCachedModels(value: unknown, pinned: readonly CatalogEntry[]): Model<Api>[] {
  if (!Array.isArray(value) || value.length > MAX_CATALOG_MODELS) {
    throw new Error("invalid cached dynamic models");
  }
  const pinnedIds = new Set(pinned.map((entry) => entry.id));
  const unique = new Set<string>();
  return value.map((raw) => {
    const id = isRecord(raw) ? raw.id : undefined;
    if (
      !isValidOpenCodeModelId(id) ||
      pinnedIds.has(id) ||
      !unique.add(id)
    ) {
      throw new Error("invalid cached dynamic model");
    }
    return validateModelSpec(raw, "opencode-go", id);
  });
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}
