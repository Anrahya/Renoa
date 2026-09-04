import {
  FILE_FIELDS,
  GOOGLE_API_ORIGIN,
  GOOGLE_UPLOAD_ORIGIN,
  MAX_JSON_RESPONSE_BYTES,
  MAX_READ_SOURCE_BYTES,
  REQUEST_TIMEOUT_MS,
} from "./constants.js";
import { DriveApiError, DriveInputError } from "./errors.js";
import {
  type JsonObject,
  requireJsonObject,
  stringField,
} from "./json.js";
import { bytesToBase64, fileId, mimeType } from "./validation.js";

export type DriveFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

export interface ListFilesInput {
  readonly query?: string;
  readonly orderBy?: string;
  readonly pageSize: number;
  readonly pageToken?: string;
}

export interface UploadInput {
  readonly title: string;
  readonly parentId?: string;
  readonly contentMimeType?: string;
  readonly driveMimeType?: string;
  readonly content?: Uint8Array;
}

const GOOGLE_NATIVE_PREFIX = "application/vnd.google-apps.";
const DEFAULT_EXPORTS: Readonly<Record<string, string>> = {
  "application/vnd.google-apps.document": "text/plain",
  "application/vnd.google-apps.presentation": "text/plain",
  "application/vnd.google-apps.spreadsheet": "text/csv",
};

export class DriveClient {
  readonly #token: string;
  readonly #signal: AbortSignal;
  readonly #fetch: DriveFetch;

  constructor(token: string, signal: AbortSignal, fetchFn: DriveFetch = fetch) {
    this.#token = token;
    this.#signal = signal;
    this.#fetch = fetchFn;
  }

  async listFiles(input: ListFilesInput): Promise<JsonObject> {
    const params = new URLSearchParams({
      pageSize: String(input.pageSize),
      spaces: "drive",
      corpora: "user",
      includeItemsFromAllDrives: "true",
      supportsAllDrives: "true",
      fields: `nextPageToken,incompleteSearch,files(${FILE_FIELDS})`,
    });
    setOptional(params, "q", input.query);
    setOptional(params, "orderBy", input.orderBy);
    setOptional(params, "pageToken", input.pageToken);
    return this.#requestJson(`/drive/v3/files?${params}`);
  }

  async getFile(rawFileId: string): Promise<JsonObject> {
    const id = fileId(rawFileId);
    const params = new URLSearchParams({ fields: FILE_FIELDS, supportsAllDrives: "true" });
    return this.#requestJson(`/drive/v3/files/${encodeURIComponent(id)}?${params}`);
  }

  async readText(rawFileId: string): Promise<{
    file: JsonObject;
    contentMimeType: string;
    text: string;
  }> {
    const file = await this.getFile(rawFileId);
    const sourceMimeType = stringField(file, "mimeType");
    if (sourceMimeType === undefined) {
      throw new DriveApiError(502, "Google Drive file metadata has no MIME type.");
    }
    const exportMimeType = DEFAULT_EXPORTS[sourceMimeType];
    let response: Response;
    let contentMimeType: string;
    if (exportMimeType !== undefined) {
      response = await this.#request(
        `/drive/v3/files/${encodeURIComponent(fileId(rawFileId))}/export?${new URLSearchParams({ mimeType: exportMimeType })}`,
      );
      contentMimeType = exportMimeType;
    } else if (isTextMimeType(sourceMimeType)) {
      response = await this.#request(
        `/drive/v3/files/${encodeURIComponent(fileId(rawFileId))}?alt=media&supportsAllDrives=true`,
      );
      contentMimeType = sourceMimeType;
    } else {
      throw new DriveInputError(
        `File MIME type ${sourceMimeType} has no lossless text representation. Use download_file_content.`,
      );
    }
    const bytes = await readBounded(response, MAX_READ_SOURCE_BYTES);
    try {
      return {
        file,
        contentMimeType,
        text: new TextDecoder("utf-8", { fatal: true }).decode(bytes),
      };
    } catch {
      throw new DriveInputError(
        `File content returned as ${contentMimeType} is not valid UTF-8. Use download_file_content.`,
      );
    }
  }

  async downloadChunk(
    rawFileId: string,
    requestedExportMimeType: string | undefined,
    byteOffset: number,
    maxBytes: number,
  ): Promise<JsonObject> {
    const id = fileId(rawFileId);
    const file = await this.getFile(id);
    const sourceMimeType = stringField(file, "mimeType");
    if (sourceMimeType === undefined) {
      throw new DriveApiError(502, "Google Drive file metadata has no MIME type.");
    }
    const native = sourceMimeType.startsWith(GOOGLE_NATIVE_PREFIX);
    const exportMimeType = requestedExportMimeType === undefined
      ? DEFAULT_EXPORTS[sourceMimeType]
      : mimeType(requestedExportMimeType);
    if (native && exportMimeType === undefined) {
      throw new DriveInputError(
        `exportMimeType is required for Google-native MIME type ${sourceMimeType}.`,
      );
    }
    let path: string;
    if (native) {
      if (exportMimeType === undefined) {
        throw new DriveInputError(
          `exportMimeType is required for Google-native MIME type ${sourceMimeType}.`,
        );
      }
      path = `/drive/v3/files/${encodeURIComponent(id)}/export?${new URLSearchParams({ mimeType: exportMimeType })}`;
    } else {
      path = `/drive/v3/files/${encodeURIComponent(id)}?alt=media&supportsAllDrives=true`;
    }
    const end = byteOffset + maxBytes - 1;
    const response = await this.#request(path, {
      headers: { Range: `bytes=${byteOffset}-${end}` },
    });
    const readLimit = response.status === 206 ? maxBytes + 1 : byteOffset + maxBytes + 1;
    const received = await readAtMost(response, readLimit);
    const range = response.status === 206
      ? requireContentRange(response.headers.get("content-range"), byteOffset, received)
      : undefined;
    const chunk = response.status === 206
      ? received.bytes.subarray(0, maxBytes)
      : received.bytes.subarray(byteOffset, byteOffset + maxBytes);
    const returnedEnd = byteOffset + chunk.byteLength;
    const complete = range === undefined
      ? !received.truncated && received.bytes.byteLength <= returnedEnd
      : range.total !== undefined && returnedEnd >= range.total;
    return {
      file,
      contentMimeType:
        exportMimeType ?? response.headers.get("content-type")?.split(";", 1)[0] ?? sourceMimeType,
      base64Content: bytesToBase64(chunk),
      byteOffset,
      returnedBytes: chunk.byteLength,
      complete,
      ...(complete ? {} : { nextByteOffset: returnedEnd }),
    };
  }

  async createFile(input: UploadInput): Promise<JsonObject> {
    const metadata: JsonObject = {
      name: input.title,
      ...(input.parentId === undefined ? {} : { parents: [fileId(input.parentId)] }),
      ...(input.driveMimeType === undefined ? {} : { mimeType: mimeType(input.driveMimeType) }),
    };
    const params = new URLSearchParams({ fields: FILE_FIELDS, supportsAllDrives: "true" });
    if (input.content === undefined) {
      return this.#requestJson(`/drive/v3/files?${params}`, {
        method: "POST",
        headers: { "Content-Type": "application/json; charset=utf-8" },
        body: JSON.stringify(metadata),
      });
    }
    if (input.contentMimeType === undefined) {
      throw new DriveInputError("contentMimeType is required when content is provided.");
    }
    const boundary = `renoa_${crypto.randomUUID().replaceAll("-", "")}`;
    const body = multipartBody(boundary, metadata, mimeType(input.contentMimeType), input.content);
    const uploadParams = new URLSearchParams({
      uploadType: "multipart",
      fields: FILE_FIELDS,
      supportsAllDrives: "true",
    });
    return this.#requestJson(`/upload/drive/v3/files?${uploadParams}`, {
      method: "POST",
      headers: { "Content-Type": `multipart/related; boundary=${boundary}` },
      body,
    }, GOOGLE_UPLOAD_ORIGIN);
  }

  async copyFile(
    rawFileId: string,
    title: string | undefined,
    parentId: string | undefined,
  ): Promise<JsonObject> {
    const id = fileId(rawFileId);
    const metadata: JsonObject = {
      ...(title === undefined ? {} : { name: title }),
      ...(parentId === undefined ? {} : { parents: [fileId(parentId)] }),
    };
    const params = new URLSearchParams({ fields: FILE_FIELDS, supportsAllDrives: "true" });
    return this.#requestJson(`/drive/v3/files/${encodeURIComponent(id)}/copy?${params}`, {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify(metadata),
    });
  }

  async listPermissions(
    rawFileId: string,
    pageSize: number,
    pageToken?: string,
  ): Promise<JsonObject> {
    const id = fileId(rawFileId);
    const params = new URLSearchParams({
      pageSize: String(pageSize),
      supportsAllDrives: "true",
      fields:
        "nextPageToken,permissions(id,type,role,emailAddress,domain,displayName,allowFileDiscovery,expirationTime,pendingOwner,deleted,permissionDetails)",
    });
    setOptional(params, "pageToken", pageToken);
    return this.#requestJson(
      `/drive/v3/files/${encodeURIComponent(id)}/permissions?${params}`,
    );
  }

  async #requestJson(
    path: string,
    init: RequestInit = {},
    origin = GOOGLE_API_ORIGIN,
  ): Promise<JsonObject> {
    const response = await this.#request(path, init, origin);
    const bytes = await readBounded(response, MAX_JSON_RESPONSE_BYTES);
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      throw new DriveApiError(502, "Google Drive returned invalid JSON.");
    }
    return requireJsonObject(value, "JSON response");
  }

  async #request(
    path: string,
    init: RequestInit = {},
    origin = GOOGLE_API_ORIGIN,
  ): Promise<Response> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    headers.set("Authorization", `Bearer ${this.#token}`);
    const signal = AbortSignal.any([
      this.#signal,
      AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    ]);
    let response: Response;
    try {
      response = await this.#fetch(new URL(path, origin), {
        ...init,
        headers,
        redirect: "manual",
        signal,
      });
    } catch (error) {
      if (signal.aborted) {
        throw new DriveApiError(504, "Google Drive request was cancelled or timed out.");
      }
      const message = error instanceof Error ? error.message.slice(0, 256) : "network failure";
      throw new DriveApiError(503, `Google Drive could not be reached: ${message}`);
    }
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel();
      throw new DriveApiError(502, "Google Drive returned an unexpected redirect.");
    }
    if (!response.ok) {
      throw await driveFailure(response);
    }
    return response;
  }
}

function setOptional(params: URLSearchParams, key: string, value: string | undefined): void {
  if (value !== undefined && value.length > 0) {
    params.set(key, value);
  }
}

function isTextMimeType(value: string): boolean {
  return value.startsWith("text/") || [
    "application/json",
    "application/ld+json",
    "application/xml",
    "application/javascript",
    "application/x-javascript",
    "application/yaml",
  ].includes(value);
}

function multipartBody(
  boundary: string,
  metadata: JsonObject,
  contentMimeType: string,
  content: Uint8Array,
): ArrayBuffer {
  const encoder = new TextEncoder();
  const prefix = encoder.encode(
    `--${boundary}\r\nContent-Type: application/json; charset=UTF-8\r\n\r\n${JSON.stringify(metadata)}\r\n--${boundary}\r\nContent-Type: ${contentMimeType}\r\n\r\n`,
  );
  const suffix = encoder.encode(`\r\n--${boundary}--\r\n`);
  const result = new Uint8Array(prefix.length + content.length + suffix.length);
  result.set(prefix, 0);
  result.set(content, prefix.length);
  result.set(suffix, prefix.length + content.length);
  return result.buffer;
}

async function driveFailure(response: Response): Promise<DriveApiError> {
  let message = `Google Drive returned HTTP ${response.status}.`;
  let reason: string | undefined;
  try {
    const bytes = await readBounded(response, 64 * 1024);
    const value: unknown = JSON.parse(new TextDecoder().decode(bytes));
    const object = requireJsonObject(value, "error response");
    const error = object.error;
    if (typeof error === "object" && error !== null && !Array.isArray(error)) {
      const candidate = Object.getOwnPropertyDescriptor(error, "message")?.value;
      if (typeof candidate === "string" && candidate.trim().length > 0) {
        message = candidate.trim().slice(0, 512);
      }
      const errors = Object.getOwnPropertyDescriptor(error, "errors")?.value;
      if (Array.isArray(errors)) {
        for (const item of errors) {
          if (typeof item !== "object" || item === null || Array.isArray(item)) continue;
          const candidateReason = Object.getOwnPropertyDescriptor(item, "reason")?.value;
          if (typeof candidateReason === "string" && candidateReason.length > 0) {
            reason = candidateReason.slice(0, 128);
            break;
          }
        }
      }
    }
  } catch {
    // Keep the status-only error when Google returns an unreadable body.
  }
  return new DriveApiError(response.status, message, reason);
}

async function readBounded(response: Response, maxBytes: number): Promise<Uint8Array> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (Number.isFinite(parsed) && parsed > maxBytes) {
      await response.body?.cancel();
      throw new DriveApiError(413, `Google Drive response exceeds ${maxBytes} bytes.`);
    }
  }
  if (response.body === null) {
    return new Uint8Array();
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  try {
    while (true) {
      const result = await reader.read();
      if (result.done) break;
      if (result.value !== undefined) {
        length += result.value.byteLength;
        if (length > maxBytes) {
          throw new DriveApiError(413, `Google Drive response exceeds ${maxBytes} bytes.`);
        }
        chunks.push(result.value);
      }
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return bytes;
}

async function readAtMost(
  response: Response,
  maxBytes: number,
): Promise<{ bytes: Uint8Array; truncated: boolean }> {
  if (response.body === null) {
    return { bytes: new Uint8Array(), truncated: false };
  }
  const reader = response.body.getReader();
  const chunks: Uint8Array[] = [];
  let length = 0;
  let truncated = false;
  try {
    while (length < maxBytes) {
      const result = await reader.read();
      if (result.done) break;
      if (result.value === undefined) continue;
      const remaining = maxBytes - length;
      const retained = result.value.subarray(0, remaining);
      chunks.push(retained);
      length += retained.byteLength;
      if (retained.byteLength < result.value.byteLength) {
        truncated = true;
        break;
      }
    }
    if (length === maxBytes && !truncated) {
      const next = await reader.read();
      truncated = !next.done;
    }
  } finally {
    await reader.cancel().catch(() => undefined);
  }
  const bytes = new Uint8Array(length);
  let offset = 0;
  for (const chunk of chunks) {
    bytes.set(chunk, offset);
    offset += chunk.byteLength;
  }
  return { bytes, truncated };
}

function requireContentRange(
  value: string | null,
  requestedStart: number,
  received: { bytes: Uint8Array; truncated: boolean },
): { total?: number } {
  const match = value === null ? null : /^bytes (\d+)-(\d+)\/(\d+|\*)$/.exec(value);
  if (match === null) {
    throw new DriveApiError(502, "Google Drive returned an invalid partial response.");
  }
  const start = Number(match[1]);
  const end = Number(match[2]);
  const expectedLength = end - start + 1;
  if (
    !Number.isSafeInteger(start) ||
    !Number.isSafeInteger(end) ||
    start !== requestedStart ||
    end < start ||
    expectedLength !== received.bytes.byteLength ||
    received.truncated
  ) {
    throw new DriveApiError(502, "Google Drive returned a mismatched partial response.");
  }
  const totalValue = match[3];
  if (totalValue === undefined || totalValue === "*") return {};
  const total = Number(totalValue);
  if (!Number.isSafeInteger(total) || total <= end) {
    throw new DriveApiError(502, "Google Drive returned an invalid partial response size.");
  }
  return { total };
}
