import { DatabaseSync } from "node:sqlite";

import { securePrivateFile } from "./private-file.js";

const SQLITE_BUSY = 5;

export interface OauthRefreshLock {
  release(): void;
}

/**
 * Tries to take a process-crash-safe refresh lock without blocking Node's event
 * loop. The lock lives in a dedicated SQLite database, so holding it across a
 * token request cannot block reads or writes in the credential database.
 */
export function tryAcquireOauthRefreshLock(storePath: string): OauthRefreshLock | undefined {
  const lockPath = `${storePath}.oauth-refresh.sqlite`;
  securePrivateFile(lockPath);
  let database: DatabaseSync | undefined;
  try {
    database = new DatabaseSync(lockPath);
    database.exec("PRAGMA busy_timeout = 0; BEGIN IMMEDIATE;");
  } catch (error) {
    database?.close();
    if (isBusy(error)) {
      return undefined;
    }
    throw error;
  }

  let released = false;
  return {
    release: () => {
      if (released) {
        return;
      }
      released = true;
      try {
        database.exec("ROLLBACK");
      } finally {
        database.close();
      }
    },
  };
}

function isBusy(error: unknown): boolean {
  return (
    error instanceof Error &&
    "errcode" in error &&
    (error as Error & { readonly errcode?: unknown }).errcode === SQLITE_BUSY
  );
}
