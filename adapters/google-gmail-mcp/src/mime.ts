import { arrayField, isJsonObject, type JsonObject, objectField, stringField } from "./json.js";
import { base64UrlDecode, base64UrlEncode, headerValue, messageBody, optionalHeaderValue } from "./validation.js";

export interface ComposeMessageInput {
  readonly to: string;
  readonly cc?: string;
  readonly bcc?: string;
  readonly subject: string;
  readonly body: string;
  readonly inReplyTo?: string;
  readonly references?: string;
}

export interface AttachmentSummary {
  readonly filename: string;
  readonly mimeType: string;
  readonly size: number | null;
  readonly attachmentId: string | null;
}

export function buildRawRfc2822(input: ComposeMessageInput): string {
  const headers: string[] = [
    `To: ${headerValue(input.to, "to")}`,
    `Subject: ${encodeHeader(headerValue(input.subject, "subject"))}`,
    "MIME-Version: 1.0",
    "Content-Type: text/plain; charset=utf-8",
    "Content-Transfer-Encoding: 8bit",
  ];
  const cc = optionalHeaderValue(input.cc, "cc");
  const bcc = optionalHeaderValue(input.bcc, "bcc");
  const inReplyTo = optionalHeaderValue(input.inReplyTo, "inReplyTo");
  const references = optionalHeaderValue(input.references, "references", 2_048);
  if (cc !== undefined) headers.push(`Cc: ${cc}`);
  if (bcc !== undefined) headers.push(`Bcc: ${bcc}`);
  if (inReplyTo !== undefined) headers.push(`In-Reply-To: ${inReplyTo}`);
  if (references !== undefined) headers.push(`References: ${references}`);

  const normalizedBody = messageBody(input.body).replace(/\r?\n/gu, "\r\n");
  const raw = `${headers.join("\r\n")}\r\n\r\n${normalizedBody}`;
  return base64UrlEncode(new TextEncoder().encode(raw));
}

export function extractHeaders(payload: JsonObject | undefined): Record<string, string> {
  const result: Record<string, string> = {};
  if (payload === undefined) return result;
  const headers = arrayField(payload, "headers");
  if (headers === undefined) return result;
  const wanted = new Set(["from", "to", "cc", "subject", "date", "message-id"]);
  for (const item of headers) {
    if (!isJsonObject(item)) continue;
    const name = stringField(item, "name")?.toLowerCase();
    const value = stringField(item, "value");
    if (name !== undefined && value !== undefined && wanted.has(name) && !(name in result)) {
      result[name] = value;
    }
  }
  return result;
}

export function formatMessage(
  message: JsonObject,
  maxCharacters: number,
): JsonObject {
  const payload = objectField(message, "payload");
  const headers = extractHeaders(payload);
  const extracted = extractBodyAndAttachments(payload);
  const bodyText = extracted.plainText ?? (extracted.htmlText !== undefined ? stripHtml(extracted.htmlText) : "");
  const truncated = bodyText.length > maxCharacters;
  const body = truncated ? bodyText.slice(0, maxCharacters) : bodyText;
  const labels = (arrayField(message, "labelIds") ?? []).filter(
    (item): item is string => typeof item === "string",
  );
  return {
    id: stringField(message, "id") ?? "",
    threadId: stringField(message, "threadId") ?? "",
    internalDate: stringField(message, "internalDate") ?? "",
    labelIds: labels,
    snippet: stringField(message, "snippet") ?? "",
    headers,
    body,
    truncated,
    attachments: extracted.attachments.map((item) => ({
      filename: item.filename,
      mimeType: item.mimeType,
      size: item.size,
      attachmentId: item.attachmentId,
    })),
  };
}

function extractBodyAndAttachments(payload: JsonObject | undefined): {
  plainText?: string;
  htmlText?: string;
  attachments: AttachmentSummary[];
} {
  const attachments: AttachmentSummary[] = [];
  const plainParts: string[] = [];
  const htmlParts: string[] = [];
  if (payload === undefined) return { attachments };

  const stack: JsonObject[] = [payload];
  while (stack.length > 0) {
    const part = stack.pop()!;
    const mimeType = (stringField(part, "mimeType") ?? "").toLowerCase();
    const filename = stringField(part, "filename") ?? "";
    const body = objectField(part, "body");
    const attachmentId = body === undefined ? undefined : stringField(body, "attachmentId");
    const rawSize = body?.size;
    const size = typeof rawSize === "number" ? rawSize : null;
    if (filename.length > 0 || attachmentId !== undefined) {
      attachments.push({
        filename,
        mimeType,
        size,
        attachmentId: attachmentId ?? null,
      });
    } else if (body !== undefined) {
      const data = stringField(body, "data");
      if (data !== undefined && data.length > 0) {
        const decoded = decodeUtf8(data);
        if (mimeType === "text/plain") {
          plainParts.push(decoded);
        } else if (mimeType === "text/html") {
          htmlParts.push(decoded);
        }
      }
    }
    const parts = arrayField(part, "parts");
    if (parts !== undefined) {
      for (let index = parts.length - 1; index >= 0; index -= 1) {
        const child = parts[index];
        if (isJsonObject(child)) {
          stack.push(child);
        }
      }
    }
  }
  return {
    ...(plainParts.length > 0 ? { plainText: plainParts.join("\n\n") } : {}),
    ...(htmlParts.length > 0 ? { htmlText: htmlParts.join("\n\n") } : {}),
    attachments,
  };
}

function decodeUtf8(base64url: string): string {
  try {
    return new TextDecoder("utf-8").decode(base64UrlDecode(base64url));
  } catch {
    return "";
  }
}

function stripHtml(html: string): string {
  return html
    .replace(/<style[\s\S]*?<\/style>/giu, " ")
    .replace(/<script[\s\S]*?<\/script>/giu, " ")
    .replace(/<(?:br|\/p|\/div|\/li|\/tr|\/h[1-6])\b[^>]*>/giu, "\n")
    .replace(/<[^>]+>/gu, " ")
    .replaceAll("&nbsp;", " ")
    .replaceAll("&amp;", "&")
    .replaceAll("&lt;", "<")
    .replaceAll("&gt;", ">")
    .replaceAll("&quot;", "\"")
    .replaceAll("&#39;", "'")
    .replace(/[ \t]+\n/gu, "\n")
    .replace(/\n{3,}/gu, "\n\n")
    .trim();
}

function encodeHeader(value: string): string {
  if (/^[\x20-\x7E]*$/u.test(value)) {
    return value;
  }
  const bytes = new TextEncoder().encode(value);
  let binary = "";
  for (const byte of bytes) {
    binary += String.fromCharCode(byte);
  }
  return `=?UTF-8?B?${btoa(binary)}?=`;
}
