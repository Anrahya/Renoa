import { DriveInputError } from "./errors.js";

const FILE_ID = /^[A-Za-z0-9_-]+$/;
const MIME_TYPE = /^[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*\/[A-Za-z0-9][A-Za-z0-9!#$&^_.+-]*$/;

export function fileId(value: string): string {
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length > 256 || !FILE_ID.test(normalized)) {
    throw new DriveInputError("fileId must be a Google Drive file ID, not a name or URL.");
  }
  return normalized;
}

export function optionalFileId(value: string | undefined): string | undefined {
  return value === undefined ? undefined : fileId(value);
}

export function mimeType(value: string): string {
  const normalized = value.trim().toLowerCase();
  if (normalized.length > 256 || !MIME_TYPE.test(normalized)) {
    throw new DriveInputError("MIME type is invalid.");
  }
  return normalized;
}

export function nonBlank(value: string, name: string, maxBytes: number): string {
  const normalized = value.trim();
  if (normalized.length === 0 || new TextEncoder().encode(normalized).byteLength > maxBytes) {
    throw new DriveInputError(`${name} must be non-empty and at most ${maxBytes} UTF-8 bytes.`);
  }
  return normalized;
}

export function canonicalBase64(value: string): Uint8Array {
  if (value.length === 0 || value.length % 4 !== 0 || !/^(?:[A-Za-z0-9+/]{4})*(?:[A-Za-z0-9+/]{2}==|[A-Za-z0-9+/]{3}=)?$/.test(value)) {
    throw new DriveInputError("base64Content must be canonical base64.");
  }
  const decoded = Uint8Array.from(atob(value), (character) => character.charCodeAt(0));
  if (bytesToBase64(decoded) !== value) {
    throw new DriveInputError("base64Content must be canonical base64.");
  }
  return decoded;
}

export function bytesToBase64(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 8_192;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary);
}
