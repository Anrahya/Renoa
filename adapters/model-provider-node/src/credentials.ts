import { realpathSync } from "node:fs";
import { setTimeout as delay } from "node:timers/promises";
import { DatabaseSync } from "node:sqlite";

import { tryAcquireOauthRefreshLock } from "./oauth-refresh-lock.js";
import { securePrivateFile } from "./private-file.js";

const SCHEMA_VERSION = 1;
const DEFAULT_BUSY_MS = 30_000;
const WAIT_POLL_MS = 50;

export type Credential =
  | {
      readonly type: "oauth";
      readonly access: string;
      readonly refresh: string;
      readonly expires: number;
      readonly accountId?: string;
    }
  | {
      readonly type: "api_key";
      readonly key?: string;
      readonly env?: Record<string, string>;
    };

export type OauthCredential = Extract<Credential, { type: "oauth" }>;

export interface CredentialInfo {
  readonly providerId: string;
  readonly type: Credential["type"];
}

export interface RefreshClock {
  sleep(ms: number): Promise<void>;
}

export interface CredentialStoreOptions {
  readonly busyTimeoutMs?: number;
  readonly clock?: RefreshClock;
}

const systemClock: RefreshClock = {
  sleep: (ms) => delay(ms),
};

export class SqliteCredentialStore {
  readonly #database: DatabaseSync;
  readonly #path: string;
  readonly #clock: RefreshClock;

  constructor(path: string, options: CredentialStoreOptions = {}) {
    securePrivateFile(path);
    this.#path = realpathSync.native(path);
    this.#database = new DatabaseSync(this.#path);
    const busyTimeoutMs = Math.max(0, Math.floor(options.busyTimeoutMs ?? DEFAULT_BUSY_MS));
    this.#database.exec(`PRAGMA busy_timeout = ${busyTimeoutMs};`);
    this.#clock = options.clock ?? systemClock;
    const schema = this.#database.prepare("PRAGMA user_version").get() as {
      readonly user_version: number;
    };
    if (schema.user_version > SCHEMA_VERSION) {
      this.#database.close();
      throw new Error(
        `credential database schema ${schema.user_version} is newer than supported version ${SCHEMA_VERSION}`,
      );
    }
    try {
      this.#database.exec(`
        PRAGMA journal_mode = DELETE;
        PRAGMA synchronous = FULL;
        CREATE TABLE IF NOT EXISTS credentials (
          provider_id TEXT PRIMARY KEY,
          credential_type TEXT NOT NULL CHECK (credential_type IN ('api_key', 'oauth')),
          credential_json TEXT NOT NULL
        ) STRICT;
        PRAGMA user_version = 1;
      `);
    } catch (error) {
      this.#database.close();
      throw error;
    }
  }

  read(providerId: string): Credential | undefined {
    const row = this.#database
      .prepare("SELECT credential_json FROM credentials WHERE provider_id = ?")
      .get(providerId) as { readonly credential_json: string } | undefined;
    return row === undefined ? undefined : decodeCredential(providerId, row.credential_json);
  }

  list(): readonly CredentialInfo[] {
    const rows = this.#database
      .prepare("SELECT provider_id, credential_type FROM credentials ORDER BY provider_id")
      .all() as Array<{
      readonly provider_id: string;
      readonly credential_type: Credential["type"];
    }>;
    return rows.map((row) => ({
      providerId: row.provider_id,
      type: row.credential_type,
    }));
  }

  write(providerId: string, credential: Credential): void {
    this.#immediate(() => {
      this.#insert(providerId, credential);
    });
  }

  delete(providerId: string): void {
    this.#immediate(() => {
      this.#database.prepare("DELETE FROM credentials WHERE provider_id = ?").run(providerId);
    });
  }

  /**
   * Refresh an OAuth credential. Ownership is a transaction held in a
   * dedicated lock database for the duration of the token request. It is not
   * time-based: a paused holder keeps ownership, while process death releases
   * the SQLite lock. A failed refresh leaves the stored credential unchanged.
   */
  async refreshOauth(
    providerId: string,
    refresh: (current: OauthCredential) => Promise<OauthCredential>,
  ): Promise<Credential | undefined> {
    const original = this.read(providerId);
    if (original === undefined || original.type !== "oauth") {
      return original;
    }
    while (true) {
      const current = this.read(providerId);
      if (current === undefined) {
        return undefined;
      }
      if (current.type !== "oauth") {
        return current;
      }
      if (!sameOauth(current, original)) {
        return current;
      }
      const lock = tryAcquireOauthRefreshLock(this.#path);
      if (lock === undefined) {
        await this.#clock.sleep(WAIT_POLL_MS);
        continue;
      }
      try {
        const locked = this.read(providerId);
        if (locked === undefined || locked.type !== "oauth") {
          return locked;
        }
        if (!sameOauth(locked, original)) {
          return locked;
        }
        const next = await refresh(locked);
        if (next.type !== "oauth") {
          throw new Error("OAuth refresh returned a non-OAuth credential");
        }
        return this.#storeIfUnchanged(providerId, locked, next);
      } finally {
        lock.release();
      }
    }
  }

  close(): void {
    this.#database.close();
  }

  #storeIfUnchanged(
    providerId: string,
    snapshot: OauthCredential,
    next: OauthCredential,
  ): Credential | undefined {
    return this.#immediate(() => {
      const stored = this.read(providerId);
      if (stored === undefined) {
        this.#insert(providerId, next);
        return next;
      }
      if (stored.type !== "oauth") {
        return stored;
      }
      if (sameOauth(stored, snapshot)) {
        this.#insert(providerId, next);
        return next;
      }
      return stored;
    });
  }

  #insert(providerId: string, credential: Credential): void {
    this.#database
      .prepare(
        `
          INSERT INTO credentials (provider_id, credential_type, credential_json)
          VALUES (?, ?, ?)
          ON CONFLICT (provider_id) DO UPDATE SET
            credential_type = excluded.credential_type,
            credential_json = excluded.credential_json
        `,
      )
      .run(providerId, credential.type, JSON.stringify(credential));
  }

  #immediate<T>(operation: () => T): T {
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const result = operation();
      this.#database.exec("COMMIT");
      return result;
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }
}

function sameOauth(left: OauthCredential, right: OauthCredential): boolean {
  return left.access === right.access && left.refresh === right.refresh && left.expires === right.expires;
}

function decodeCredential(providerId: string, encoded: string): Credential {
  let value: unknown;
  try {
    value = JSON.parse(encoded);
  } catch {
    throw invalidCredential(providerId);
  }
  if (!isRecord(value)) {
    throw invalidCredential(providerId);
  }
  if (
    value.type === "oauth" &&
    isNonEmptyString(value.access) &&
    isNonEmptyString(value.refresh) &&
    typeof value.expires === "number" &&
    Number.isFinite(value.expires)
  ) {
    const credential: OauthCredential = {
      type: "oauth",
      access: value.access,
      refresh: value.refresh,
      expires: value.expires,
    };
    if (typeof value.accountId === "string") {
      return { ...credential, accountId: value.accountId };
    }
    return credential;
  }
  if (value.type === "api_key") {
    const credential: Extract<Credential, { type: "api_key" }> = { type: "api_key" };
    if (value.key === undefined) {
      return value.env !== undefined && isStringRecord(value.env)
        ? { ...credential, env: value.env }
        : credential;
    }
    if (typeof value.key !== "string") {
      throw invalidCredential(providerId);
    }
    return value.env !== undefined && isStringRecord(value.env)
      ? { type: "api_key", key: value.key, env: value.env }
      : { type: "api_key", key: value.key };
  }
  throw invalidCredential(providerId);
}

function isRecord(value: unknown): value is Record<string, unknown> {
  return typeof value === "object" && value !== null && !Array.isArray(value);
}

function isStringRecord(value: unknown): value is Record<string, string> {
  return isRecord(value) && Object.values(value).every((entry) => typeof entry === "string");
}

function isNonEmptyString(value: unknown): value is string {
  return typeof value === "string" && value.length > 0;
}

function invalidCredential(providerId: string): Error {
  return new Error(`stored credential for ${providerId} is invalid`);
}
