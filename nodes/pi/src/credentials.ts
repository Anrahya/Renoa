import { chmodSync, closeSync, lstatSync, openSync } from "node:fs";
import { DatabaseSync } from "node:sqlite";

import type {
  AuthOperationOptions,
  Credential,
  CredentialInfo,
  CredentialStore,
} from "@earendil-works/pi-ai";

const SCHEMA_VERSION = 1;

export class SqliteCredentialStore implements CredentialStore {
  readonly #database: DatabaseSync;
  #tail: Promise<void> = Promise.resolve();

  constructor(path: string) {
    secureFile(path);
    this.#database = new DatabaseSync(path);
    const schema = this.#database.prepare("PRAGMA user_version").get() as {
      readonly user_version: number;
    };
    if (schema.user_version > SCHEMA_VERSION) {
      this.#database.close();
      throw new Error(
        `Pi credential database schema ${schema.user_version} is newer than supported version ${SCHEMA_VERSION}`,
      );
    }
    try {
      this.#database.exec(`
        PRAGMA journal_mode = DELETE;
        PRAGMA synchronous = FULL;
        PRAGMA busy_timeout = 5000;
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

  async read(
    providerId: string,
    options?: AuthOperationOptions,
  ): Promise<Credential | undefined> {
    options?.signal?.throwIfAborted();
    const row = this.#database
      .prepare("SELECT credential_json FROM credentials WHERE provider_id = ?")
      .get(providerId) as { readonly credential_json: string } | undefined;
    return row === undefined ? undefined : decodeCredential(providerId, row.credential_json);
  }

  async list(options?: AuthOperationOptions): Promise<readonly CredentialInfo[]> {
    options?.signal?.throwIfAborted();
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

  async modify(
    providerId: string,
    update: (current: Credential | undefined) => Promise<Credential | undefined>,
    options?: AuthOperationOptions,
  ): Promise<Credential | undefined> {
    return this.#enqueue(async () => {
      options?.signal?.throwIfAborted();
      return this.#transaction(async () => {
        const current = await this.read(providerId, options);
        const next = await update(current);
        options?.signal?.throwIfAborted();
        if (next === undefined) {
          return current;
        }
        this.#database
          .prepare(`
            INSERT INTO credentials (provider_id, credential_type, credential_json)
            VALUES (?, ?, ?)
            ON CONFLICT (provider_id) DO UPDATE SET
              credential_type = excluded.credential_type,
              credential_json = excluded.credential_json
          `)
          .run(providerId, next.type, JSON.stringify(next));
        return next;
      });
    });
  }

  async delete(providerId: string, options?: AuthOperationOptions): Promise<void> {
    await this.#enqueue(async () => {
      options?.signal?.throwIfAborted();
      await this.#transaction(async () => {
        this.#database.prepare("DELETE FROM credentials WHERE provider_id = ?").run(providerId);
      });
    });
  }

  close(): void {
    this.#database.close();
  }

  #enqueue<T>(operation: () => Promise<T>): Promise<T> {
    const pending = this.#tail.then(operation, operation);
    this.#tail = pending.then(
      () => undefined,
      () => undefined,
    );
    return pending;
  }

  async #transaction<T>(operation: () => Promise<T>): Promise<T> {
    this.#database.exec("BEGIN IMMEDIATE");
    try {
      const result = await operation();
      this.#database.exec("COMMIT");
      return result;
    } catch (error) {
      this.#database.exec("ROLLBACK");
      throw error;
    }
  }
}

function secureFile(path: string): void {
  try {
    const metadata = lstatSync(path);
    if (!metadata.isFile()) {
      throw new Error("Pi credential store must be a regular file");
    }
    chmodSync(path, 0o600);
  } catch (error) {
    if (!isMissing(error)) {
      throw error;
    }
    closeSync(openSync(path, "wx+", 0o600));
  }
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
    return value as Credential;
  }
  if (
    value.type === "api_key" &&
    (value.key === undefined || typeof value.key === "string") &&
    (value.env === undefined || isStringRecord(value.env))
  ) {
    return value as Credential;
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

function isMissing(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}
