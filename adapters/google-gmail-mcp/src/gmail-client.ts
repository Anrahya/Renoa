import {
  GOOGLE_GMAIL_ORIGIN,
  MAX_HYDRATED_THREADS,
  MAX_JSON_RESPONSE_BYTES,
  REQUEST_TIMEOUT_MS,
} from "./constants.js";
import { GmailApiError } from "./errors.js";
import {
  arrayField,
  isJsonObject,
  type JsonObject,
  objectField,
  requireJsonObject,
  stringField,
} from "./json.js";
import {
  type ComposeMessageInput,
  buildRawRfc2822,
  extractHeaders,
  formatMessage,
} from "./mime.js";
import { gmailId, labelId } from "./validation.js";

export type GmailFetch = (
  input: RequestInfo | URL,
  init?: RequestInit,
) => Promise<Response>;

export interface SearchThreadsInput {
  readonly query?: string;
  readonly labelIds?: readonly string[];
  readonly includeSpamTrash: boolean;
  readonly maxResults: number;
  readonly pageToken?: string;
  readonly includeMetadata: boolean;
}

export interface ComposeInput extends ComposeMessageInput {
  readonly threadId?: string;
}

const defaultFetch: GmailFetch = (input, init) => fetch(input, init);

export class GmailClient {
  readonly #token: string;
  readonly #signal: AbortSignal;
  readonly #fetch: GmailFetch;

  constructor(token: string, signal: AbortSignal, fetchFn: GmailFetch = defaultFetch) {
    this.#token = token;
    this.#signal = signal;
    this.#fetch = fetchFn;
  }

  async searchThreads(input: SearchThreadsInput): Promise<JsonObject> {
    const params = new URLSearchParams({
      maxResults: String(input.maxResults),
      includeSpamTrash: String(input.includeSpamTrash),
    });
    if (input.query !== undefined && input.query.trim().length > 0) {
      params.set("q", input.query.trim());
    }
    if (input.pageToken !== undefined && input.pageToken.length > 0) {
      params.set("pageToken", input.pageToken);
    }
    for (const id of input.labelIds ?? []) {
      params.append("labelIds", labelId(id));
    }
    const listing = await this.#requestJson(`/gmail/v1/users/me/threads?${params}`);
    const rawThreads = (arrayField(listing, "threads") ?? []).filter(isJsonObject);
    const shouldHydrate =
      input.includeMetadata && rawThreads.length > 0 && rawThreads.length <= MAX_HYDRATED_THREADS;
    const threads: JsonObject[] = shouldHydrate
      ? await Promise.all(rawThreads.map((thread) => this.#hydrateThreadSummary(thread)))
      : rawThreads;

    return {
      threads,
      ...(typeof listing.resultSizeEstimate === "number"
        ? { resultSizeEstimate: listing.resultSizeEstimate }
        : {}),
      ...(stringField(listing, "nextPageToken") !== undefined
        ? { nextPageToken: stringField(listing, "nextPageToken")! }
        : {}),
    };
  }

  async getThread(rawThreadId: string, maxCharacters: number): Promise<JsonObject> {
    const id = gmailId(rawThreadId, "threadId");
    const thread = await this.#requestJson(
      `/gmail/v1/users/me/threads/${encodeURIComponent(id)}?format=full`,
    );
    const rawMessages = (arrayField(thread, "messages") ?? []).filter(isJsonObject);
    const perMessageLimit = Math.max(
      1_000,
      Math.floor(maxCharacters / Math.max(1, rawMessages.length)),
    );
    let remaining = maxCharacters;
    const messages = rawMessages.map((message) => {
      const budget = Math.max(500, Math.min(perMessageLimit, remaining));
      const formatted = formatMessage(message, budget);
      const body = stringField(formatted, "body") ?? "";
      remaining = Math.max(0, remaining - body.length);
      return formatted;
    });
    return {
      id: stringField(thread, "id") ?? id,
      ...(stringField(thread, "historyId") !== undefined
        ? { historyId: stringField(thread, "historyId")! }
        : {}),
      messageCount: messages.length,
      messages,
    };
  }

  async getMessage(rawMessageId: string, maxCharacters: number): Promise<JsonObject> {
    const id = gmailId(rawMessageId, "messageId");
    const message = await this.#requestJson(
      `/gmail/v1/users/me/messages/${encodeURIComponent(id)}?format=full`,
    );
    return formatMessage(message, maxCharacters);
  }

  async listLabels(): Promise<JsonObject> {
    return this.#requestJson("/gmail/v1/users/me/labels");
  }

  async modifyThreadLabels(
    rawThreadId: string,
    addLabelIds: readonly string[],
    removeLabelIds: readonly string[],
  ): Promise<JsonObject> {
    const id = gmailId(rawThreadId, "threadId");
    const body = {
      addLabelIds: addLabelIds.map(labelId),
      removeLabelIds: removeLabelIds.map(labelId),
    };
    return this.#requestJson(`/gmail/v1/users/me/threads/${encodeURIComponent(id)}/modify`, {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify(body),
    });
  }

  async trashThread(rawThreadId: string): Promise<JsonObject> {
    const id = gmailId(rawThreadId, "threadId");
    return this.#requestJson(`/gmail/v1/users/me/threads/${encodeURIComponent(id)}/trash`, {
      method: "POST",
    });
  }

  async listDrafts(
    query: string | undefined,
    maxResults: number,
    pageToken: string | undefined,
  ): Promise<JsonObject> {
    const params = new URLSearchParams({ maxResults: String(maxResults) });
    if (query !== undefined && query.trim().length > 0) {
      params.set("q", query.trim());
    }
    if (pageToken !== undefined && pageToken.length > 0) {
      params.set("pageToken", pageToken);
    }
    return this.#requestJson(`/gmail/v1/users/me/drafts?${params}`);
  }

  async createDraft(input: ComposeInput): Promise<JsonObject> {
    const raw = buildRawRfc2822(input);
    const message: JsonObject = {
      raw,
      ...(input.threadId === undefined ? {} : { threadId: gmailId(input.threadId, "threadId") }),
    };
    return this.#requestJson("/gmail/v1/users/me/drafts", {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify({ message }),
    });
  }

  async sendDraft(rawDraftId: string): Promise<JsonObject> {
    const id = gmailId(rawDraftId, "draftId");
    return this.#requestJson("/gmail/v1/users/me/drafts/send", {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify({ id }),
    });
  }

  async sendMessage(input: ComposeInput): Promise<JsonObject> {
    const raw = buildRawRfc2822(input);
    const payload: JsonObject = {
      raw,
      ...(input.threadId === undefined ? {} : { threadId: gmailId(input.threadId, "threadId") }),
    };
    return this.#requestJson("/gmail/v1/users/me/messages/send", {
      method: "POST",
      headers: { "Content-Type": "application/json; charset=utf-8" },
      body: JSON.stringify(payload),
    });
  }

  async #hydrateThreadSummary(summary: JsonObject): Promise<JsonObject> {
    const id = stringField(summary, "id");
    if (id === undefined) return summary;
    const params = new URLSearchParams({ format: "metadata" });
    for (const header of ["From", "To", "Subject", "Date"]) {
      params.append("metadataHeaders", header);
    }
    const detail = await this.#requestJson(
      `/gmail/v1/users/me/threads/${encodeURIComponent(id)}?${params}`,
    );
    const messages = (arrayField(detail, "messages") ?? []).filter(isJsonObject);
    const latest = messages.at(-1);
    const headers = extractHeaders(latest === undefined ? undefined : objectField(latest, "payload"));
    const labelSet = new Set<string>();
    for (const message of messages) {
      for (const label of arrayField(message, "labelIds") ?? []) {
        if (typeof label === "string") labelSet.add(label);
      }
    }
    return {
      id,
      snippet: stringField(summary, "snippet") ?? "",
      messageCount: messages.length,
      labelIds: [...labelSet],
      headers,
      ...(latest !== undefined && stringField(latest, "internalDate") !== undefined
        ? { internalDate: stringField(latest, "internalDate")! }
        : {}),
    };
  }

  async #requestJson(path: string, init: RequestInit = {}): Promise<JsonObject> {
    const headers = new Headers(init.headers);
    headers.set("Accept", "application/json");
    headers.set("Authorization", `Bearer ${this.#token}`);
    const signal = AbortSignal.any([
      this.#signal,
      AbortSignal.timeout(REQUEST_TIMEOUT_MS),
    ]);
    let response: Response;
    const fetchFn = this.#fetch;
    try {
      response = await fetchFn(new URL(path, GOOGLE_GMAIL_ORIGIN), {
        ...init,
        headers,
        redirect: "manual",
        signal,
      });
    } catch (error) {
      if (signal.aborted) {
        throw new GmailApiError(504, "Google Gmail request was cancelled or timed out.");
      }
      const message = error instanceof Error ? error.message.slice(0, 256) : "network failure";
      throw new GmailApiError(503, `Google Gmail could not be reached: ${message}`);
    }
    if (response.status >= 300 && response.status < 400) {
      await response.body?.cancel();
      throw new GmailApiError(502, "Google Gmail returned an unexpected redirect.");
    }
    if (!response.ok) {
      throw await gmailFailure(response);
    }
    const bytes = await readBounded(response, MAX_JSON_RESPONSE_BYTES);
    let value: unknown;
    try {
      value = JSON.parse(new TextDecoder().decode(bytes));
    } catch {
      throw new GmailApiError(502, "Google Gmail returned invalid JSON.");
    }
    return requireJsonObject(value, "JSON response");
  }
}

async function gmailFailure(response: Response): Promise<GmailApiError> {
  let message = `Google Gmail returned HTTP ${response.status}.`;
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
    // Keep status-only error when response body is unreadable.
  }
  return new GmailApiError(response.status, message, reason);
}

async function readBounded(response: Response, maxBytes: number): Promise<Uint8Array> {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (Number.isFinite(parsed) && parsed > maxBytes) {
      await response.body?.cancel();
      throw new GmailApiError(413, `Google Gmail response exceeds ${maxBytes} bytes.`);
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
          throw new GmailApiError(413, `Google Gmail response exceeds ${maxBytes} bytes.`);
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
