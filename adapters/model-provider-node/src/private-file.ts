import { chmodSync, closeSync, lstatSync, mkdirSync, openSync } from "node:fs";
import { dirname } from "node:path";

export function securePrivateFile(path: string): void {
  try {
    const metadata = lstatSync(path);
    if (!metadata.isFile()) {
      throw new Error(`private file path is not a regular file: ${path}`);
    }
    chmodSync(path, 0o600);
    return;
  } catch (error) {
    if (!isMissing(error)) {
      throw error;
    }
  }
  createPrivateDirectoryIfMissing(dirname(path));
  try {
    closeSync(openSync(path, "wx+", 0o600));
  } catch (error) {
    if (!isExist(error)) {
      throw error;
    }
    const metadata = lstatSync(path);
    if (!metadata.isFile()) {
      throw new Error(`private file path is not a regular file: ${path}`);
    }
    chmodSync(path, 0o600);
  }
}

function createPrivateDirectoryIfMissing(path: string): void {
  try {
    const metadata = lstatSync(path);
    if (!metadata.isDirectory()) {
      throw new Error(`private file parent is not a directory: ${path}`);
    }
    return;
  } catch (error) {
    if (!isMissing(error)) {
      throw error;
    }
  }
  const parent = dirname(path);
  if (parent !== path) {
    createPrivateDirectoryIfMissing(parent);
  }
  try {
    mkdirSync(path, { mode: 0o700 });
  } catch (error) {
    if (!isExist(error)) {
      throw error;
    }
    const metadata = lstatSync(path);
    if (!metadata.isDirectory()) {
      throw new Error(`private file parent is not a directory: ${path}`);
    }
    return;
  }
  chmodSync(path, 0o700);
}

function isExist(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === "EEXIST";
}

function isMissing(error: unknown): error is NodeJS.ErrnoException {
  return error instanceof Error && "code" in error && error.code === "ENOENT";
}
