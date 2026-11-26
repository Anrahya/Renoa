import { MAX_BODY_BYTES } from "./constants.js";
import { GmailInputError } from "./errors.js";

const GMAIL_ID = /^[A-Za-z0-9_-]+$/;
const LABEL_ID = /^[A-Za-z0-9_.-]+$/;

export function gmailId(value: string, field: string): string {
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length > 128 || !GMAIL_ID.test(normalized)) {
    throw new GmailInputError(`${field} must be a valid Gmail ID.`);
  }
  return normalized;
}

export function labelId(value: string): string {
  const normalized = value.trim();
  if (normalized.length === 0 || normalized.length > 128 || !LABEL_ID.test(normalized)) {
    throw new GmailInputError("labelId must be a valid Gmail label ID.");
  }
  return normalized;
}

export function headerValue(value: string, name: string, maxChars = 1_024): string {
  const normalized = value.trim();
  if (
    normalized.length === 0 ||
    normalized.length > maxChars ||
    /[\r\n\u0000]/u.test(normalized)
  ) {
    throw new GmailInputError(
      `${name} must be a single non-empty line without control characters.`,
    );
  }
  return normalized;
}

export function optionalHeaderValue(
  value: string | undefined,
  name: string,
  maxChars = 1_024,
): string | undefined {
  return value === undefined ? undefined : headerValue(value, name, maxChars);
}

export function messageBody(value: string): string {
  const bytes = new TextEncoder().encode(value).byteLength;
  if (bytes > MAX_BODY_BYTES) {
    throw new GmailInputError(`body must be at most ${MAX_BODY_BYTES} UTF-8 bytes.`);
  }
  return value;
}

export function base64UrlEncode(bytes: Uint8Array): string {
  let binary = "";
  const chunkSize = 8_192;
  for (let offset = 0; offset < bytes.length; offset += chunkSize) {
    binary += String.fromCharCode(...bytes.subarray(offset, offset + chunkSize));
  }
  return btoa(binary).replaceAll("+", "-").replaceAll("/", "_").replace(/=+$/u, "");
}

export function base64UrlDecode(encoded: string): Uint8Array {
  const standard = encoded.replaceAll("-", "+").replaceAll("_", "/");
  const padding = (4 - (standard.length % 4)) % 4;
  const binary = atob(standard + "=".repeat(padding));
  const bytes = new Uint8Array(binary.length);
  for (let index = 0; index < binary.length; index += 1) {
    bytes[index] = binary.charCodeAt(index);
  }
  return bytes;
}
