import { McpServer, type McpRequestContext } from "@modelcontextprotocol/server";
import { z } from "zod";

import { DEFAULT_READ_CHARACTERS, MAX_READ_CHARACTERS } from "./constants.js";
import { publicToolError } from "./errors.js";
import { GmailClient, type GmailFetch } from "./gmail-client.js";
import type { JsonObject } from "./json.js";

type ToolResult = {
  content: [{ type: "text"; text: string }];
  structuredContent: JsonObject;
  isError?: boolean;
};

const readOnly = {
  readOnlyHint: true,
  destructiveHint: false,
  idempotentHint: true,
  openWorldHint: true,
} as const;

const mutating = {
  readOnlyHint: false,
  destructiveHint: false,
  idempotentHint: false,
  openWorldHint: true,
} as const;

const gmailIdSchema = z.string().min(1).max(128);
const pageTokenSchema = z.string().min(1).max(4_096).optional();
const composeSchema = z.object({
  to: z.string().min(1).max(1_024).describe("Recipient email address(es), comma-separated."),
  cc: z.string().min(1).max(1_024).optional(),
  bcc: z.string().min(1).max(1_024).optional(),
  subject: z.string().min(1).max(1_024),
  body: z.string().max(200 * 1024).describe("Plain-text email body."),
  threadId: gmailIdSchema.optional().describe("Existing Gmail thread ID when replying."),
  inReplyTo: z.string().min(1).max(1_024).optional().describe("Message-ID header of the message being replied to."),
  references: z.string().min(1).max(2_048).optional().describe("Space-separated Message-ID chain for threading."),
});

export function createGmailServer(
  context: McpRequestContext,
  fetchFn: GmailFetch = fetch,
): McpServer {
  const request = context.requestInfo;
  const token = request === undefined ? undefined : bearerToken(request.headers);
  if (request === undefined || token === undefined) {
    throw new Error("Authenticated HTTP request context is required.");
  }
  const gmail = new GmailClient(token, request.signal, fetchFn);
  const server = new McpServer({ name: "renoa-google-gmail", version: "0.1.0" });

  server.registerTool(
    "search_threads",
    {
      description:
        "Search or list Gmail threads using standard Gmail query syntax (e.g. 'is:unread', 'from:alice@example.com', 'newer_than:7d'). Returns thread IDs, snippets, labels, and From/To/Subject/Date headers by default.",
      inputSchema: z.object({
        query: z.string().max(4_096).optional().describe("Gmail search query string. Omit to list recent inbox/mailbox threads."),
        labelIds: z.array(z.string().min(1).max(128)).max(20).optional().describe("Filter to threads carrying all specified label IDs (e.g. ['INBOX'])."),
        includeSpamTrash: z.boolean().default(false),
        maxResults: z.number().int().min(1).max(50).default(10),
        pageToken: pageTokenSchema,
        includeMetadata: z.boolean().default(true).describe("Fetch From/To/Subject/Date headers for up to 20 threads in the same call."),
      }),
      annotations: readOnly,
    },
    (args) =>
      run(token, () =>
        gmail.searchThreads({
          ...(args.query === undefined ? {} : { query: args.query }),
          ...(args.labelIds === undefined ? {} : { labelIds: args.labelIds }),
          includeSpamTrash: args.includeSpamTrash,
          maxResults: args.maxResults,
          ...(args.pageToken === undefined ? {} : { pageToken: args.pageToken }),
          includeMetadata: args.includeMetadata,
        }),
      ),
  );

  server.registerTool(
    "get_thread",
    {
      description:
        "Read all messages in a Gmail thread with decoded plain-text bodies, headers, labels, and attachment metadata.",
      inputSchema: z.object({
        threadId: gmailIdSchema.describe("Exact Gmail thread ID from search_threads."),
        maxCharacters: z.number().int().min(1_000).max(MAX_READ_CHARACTERS).default(DEFAULT_READ_CHARACTERS),
      }),
      annotations: readOnly,
    },
    ({ threadId, maxCharacters }) => run(token, () => gmail.getThread(threadId, maxCharacters)),
  );

  server.registerTool(
    "get_message",
    {
      description:
        "Read one Gmail message by ID with decoded plain-text body, headers, labels, and attachment metadata.",
      inputSchema: z.object({
        messageId: gmailIdSchema.describe("Exact Gmail message ID."),
        maxCharacters: z.number().int().min(1_000).max(MAX_READ_CHARACTERS).default(DEFAULT_READ_CHARACTERS),
      }),
      annotations: readOnly,
    },
    ({ messageId, maxCharacters }) => run(token, () => gmail.getMessage(messageId, maxCharacters)),
  );

  server.registerTool(
    "list_labels",
    {
      description: "List Gmail system and user labels with their exact IDs and names.",
      inputSchema: z.object({}),
      annotations: readOnly,
    },
    () => run(token, () => gmail.listLabels()),
  );

  server.registerTool(
    "modify_thread_labels",
    {
      description:
        "Add or remove Gmail labels on a thread (for example, remove 'UNREAD' to mark read, remove 'INBOX' to archive, or add 'STARRED').",
      inputSchema: z.object({
        threadId: gmailIdSchema,
        addLabelIds: z.array(z.string().min(1).max(128)).max(50).default([]),
        removeLabelIds: z.array(z.string().min(1).max(128)).max(50).default([]),
      }),
      annotations: mutating,
    },
    ({ threadId, addLabelIds, removeLabelIds }) =>
      run(token, () => gmail.modifyThreadLabels(threadId, addLabelIds, removeLabelIds)),
  );

  server.registerTool(
    "trash_thread",
    {
      description: "Move a Gmail thread to Trash (recoverable, not permanently deleted).",
      inputSchema: z.object({ threadId: gmailIdSchema }),
      annotations: mutating,
    },
    ({ threadId }) => run(token, () => gmail.trashThread(threadId)),
  );

  server.registerTool(
    "list_drafts",
    {
      description: "List saved Gmail drafts.",
      inputSchema: z.object({
        query: z.string().max(4_096).optional(),
        maxResults: z.number().int().min(1).max(50).default(20),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    ({ query, maxResults, pageToken }) =>
      run(token, () => gmail.listDrafts(query, maxResults, pageToken)),
  );

  server.registerTool(
    "create_draft",
    {
      description:
        "Create a Gmail draft (optionally attached to a threadId as a reply). Use this when the user wants to review before sending.",
      inputSchema: composeSchema,
      annotations: mutating,
    },
    (args) =>
      run(token, () =>
        gmail.createDraft({
          to: args.to,
          subject: args.subject,
          body: args.body,
          ...(args.cc === undefined ? {} : { cc: args.cc }),
          ...(args.bcc === undefined ? {} : { bcc: args.bcc }),
          ...(args.threadId === undefined ? {} : { threadId: args.threadId }),
          ...(args.inReplyTo === undefined ? {} : { inReplyTo: args.inReplyTo }),
          ...(args.references === undefined ? {} : { references: args.references }),
        }),
      ),
  );

  server.registerTool(
    "send_draft",
    {
      description: "Send an existing Gmail draft by its draftId.",
      inputSchema: z.object({ draftId: gmailIdSchema }),
      annotations: mutating,
    },
    ({ draftId }) => run(token, () => gmail.sendDraft(draftId)),
  );

  server.registerTool(
    "send_message",
    {
      description: "Send a new email or reply directly from the user's Gmail account.",
      inputSchema: composeSchema,
      annotations: mutating,
    },
    (args) =>
      run(token, () =>
        gmail.sendMessage({
          to: args.to,
          subject: args.subject,
          body: args.body,
          ...(args.cc === undefined ? {} : { cc: args.cc }),
          ...(args.bcc === undefined ? {} : { bcc: args.bcc }),
          ...(args.threadId === undefined ? {} : { threadId: args.threadId }),
          ...(args.inReplyTo === undefined ? {} : { inReplyTo: args.inReplyTo }),
          ...(args.references === undefined ? {} : { references: args.references }),
        }),
      ),
  );

  return server;
}

export function bearerToken(headers: Headers | Record<string, string | string[] | undefined>): string | undefined {
  const raw = headers instanceof Headers
    ? headers.get("authorization")
    : headerFromRecord(headers, "authorization");
  if (raw === null || raw === undefined) return undefined;
  const match = /^Bearer[ \t]+([^\s]+)$/i.exec(raw.trim());
  const token = match?.[1];
  if (token === undefined || token.length === 0 || token.length > 16_384 || /[\u0000-\u001F\u007F]/u.test(token)) {
    return undefined;
  }
  return token;
}

function headerFromRecord(
  headers: Record<string, string | string[] | undefined>,
  name: string,
): string | undefined {
  for (const [key, value] of Object.entries(headers)) {
    if (key.toLowerCase() !== name) continue;
    return Array.isArray(value) ? value[0] : value;
  }
  return undefined;
}

async function run(token: string, action: () => Promise<JsonObject>): Promise<ToolResult> {
  try {
    const result = await action();
    return {
      content: [{ type: "text", text: JSON.stringify(result) }],
      structuredContent: result,
    };
  } catch (error) {
    const failure = publicToolError(error, token);
    return {
      isError: true,
      content: [{ type: "text", text: JSON.stringify(failure) }],
      structuredContent: failure,
    };
  }
}
