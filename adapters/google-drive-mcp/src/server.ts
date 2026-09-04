import { McpServer, type McpRequestContext } from "@modelcontextprotocol/server";
import { z } from "zod";

import {
  DEFAULT_DOWNLOAD_BYTES,
  DEFAULT_READ_CHARACTERS,
  MAX_DOWNLOAD_BYTES,
  MAX_READ_CHARACTERS,
  MAX_UPLOAD_BYTES,
} from "./constants.js";
import { DriveClient, type DriveFetch } from "./drive-client.js";
import { DriveInputError, publicToolError } from "./errors.js";
import type { JsonObject, JsonValue } from "./json.js";
import {
  canonicalBase64,
  fileId,
  mimeType,
  nonBlank,
} from "./validation.js";

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

const fileIdSchema = z
  .string()
  .min(1)
  .max(256)
  .describe("Exact Google Drive file ID from a prior tool result. Never pass a file name or URL.");

const pageTokenSchema = z
  .string()
  .min(1)
  .max(4_096)
  .optional()
  .describe("Opaque nextPageToken returned by the previous call. Omit for the first page.");

export function createDriveServer(
  context: McpRequestContext,
  fetchFn: DriveFetch = fetch,
): McpServer {
  const request = context.requestInfo;
  const token = request === undefined ? undefined : bearerToken(request.headers);
  if (request === undefined || token === undefined) {
    throw new Error("Authenticated HTTP request context is required.");
  }
  const drive = new DriveClient(token, request.signal, fetchFn);
  const server = new McpServer({ name: "renoa-google-drive", version: "0.1.0" });

  server.registerTool(
    "list_recent_files",
    {
      description:
        "List non-trashed Drive files ordered by recent activity. Use this when the user refers to something they edited or opened recently. Results include exact file IDs. Paginate with nextPageToken.",
      inputSchema: z.object({
        orderBy: z
          .enum(["recency", "lastModified", "lastModifiedByMe", "lastViewedByMe"])
          .default("recency")
          .describe("Which recent timestamp to sort by, newest first."),
        pageSize: z.number().int().min(1).max(100).default(10),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    ({ orderBy, pageSize, pageToken }) => run(token, async () => {
      const value = await drive.listFiles({
        orderBy: recentOrder(orderBy),
        query: "trashed = false",
        pageSize,
        ...(pageToken === undefined ? {} : { pageToken }),
      });
      return value;
    }),
  );

  server.registerTool(
    "search_files",
    {
      description:
        "Search non-trashed Drive files. Use Google Drive query syntax: name contains 'term', fullText contains 'term', mimeType = '...', 'folderId' in parents, modifiedTime > 'RFC3339', sharedWithMe, and clauses joined with and/or/not. Search first when only a name is known; never invent a file ID. Paginate with nextPageToken.",
      inputSchema: z.object({
        query: z
          .string()
          .trim()
          .min(1)
          .max(8_192)
          .describe("Google Drive files.list q expression, without a trashed clause."),
        orderBy: z
          .string()
          .trim()
          .min(1)
          .max(256)
          .default("modifiedTime desc")
          .describe("Google Drive orderBy expression, such as modifiedTime desc or name."),
        pageSize: z.number().int().min(1).max(100).default(20),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    ({ query, orderBy, pageSize, pageToken }) => run(token, async () => {
      const value = await drive.listFiles({
        query: `trashed = false and (${query})`,
        orderBy,
        pageSize,
        ...(pageToken === undefined ? {} : { pageToken }),
      });
      return value;
    }),
  );

  server.registerTool(
    "get_file_metadata",
    {
      description:
        "Get metadata and capabilities for one exact Drive file ID. Search first if the user supplied only a title or description.",
      inputSchema: z.object({ fileId: fileIdSchema }),
      annotations: readOnly,
    },
    ({ fileId: id }) => run(token, () => drive.getFile(id)),
  );

  server.registerTool(
    "read_file_content",
    {
      description:
        "Read UTF-8 text from a known Drive file. Google Docs and Slides export as plain text; Sheets export as CSV. Plain text, JSON, XML, JavaScript, and YAML files download directly. Binary Office files, PDFs, and images are not extracted; use download_file_content for their bytes. Large text is paged by character offset.",
      inputSchema: z.object({
        fileId: fileIdSchema,
        startCharacter: z.number().int().min(0).max(4_000_000).default(0),
        maxCharacters: z
          .number()
          .int()
          .min(1)
          .max(MAX_READ_CHARACTERS)
          .default(DEFAULT_READ_CHARACTERS),
      }),
      annotations: readOnly,
    },
    ({ fileId: id, startCharacter, maxCharacters }) => run(token, async () => {
      const value = await drive.readText(id);
      const content = [...value.text];
      const selected = content.slice(startCharacter, startCharacter + maxCharacters).join("");
      const nextCharacter = startCharacter + [...selected].length;
      return {
        file: value.file,
        contentMimeType: value.contentMimeType,
        startCharacter,
        content: selected,
        complete: nextCharacter >= content.length,
        ...(nextCharacter >= content.length ? {} : { nextCharacter }),
      };
    }),
  );

  server.registerTool(
    "download_file_content",
    {
      description:
        "Download a bounded base64 byte chunk from one exact Drive file ID. Use this for binary files or exact bytes. For Google-native files, pass an export MIME type if no text default exists. Continue with nextByteOffset until complete; never guess offsets.",
      inputSchema: z.object({
        fileId: fileIdSchema,
        exportMimeType: z
          .string()
          .trim()
          .min(3)
          .max(256)
          .optional()
          .describe("Export MIME type for Google-native files; ignored for uploaded binary files."),
        byteOffset: z.number().int().min(0).max(16 * 1024 * 1024).default(0),
        maxBytes: z
          .number()
          .int()
          .min(1)
          .max(MAX_DOWNLOAD_BYTES)
          .default(DEFAULT_DOWNLOAD_BYTES),
      }),
      annotations: readOnly,
    },
    ({ fileId: id, exportMimeType, byteOffset, maxBytes }) =>
      run(token, () => drive.downloadChunk(id, exportMimeType, byteOffset, maxBytes)),
  );

  server.registerTool(
    "create_file",
    {
      description:
        "Create a Drive file or folder. Provide either textContent or canonical base64Content, never both. contentMimeType is required with content. driveMimeType controls the stored type; use application/vnd.google-apps.folder for a folder or a Google-native MIME type to import content as Docs, Sheets, or Slides. Omit both content fields for an empty file or folder.",
      inputSchema: z
        .object({
          title: z.string().min(1).max(1_024),
          parentId: fileIdSchema.optional(),
          textContent: z.string().max(MAX_UPLOAD_BYTES).optional(),
          base64Content: z.string().max(4 * Math.ceil(MAX_UPLOAD_BYTES / 3)).optional(),
          contentMimeType: z.string().trim().min(3).max(256).optional(),
          driveMimeType: z
            .string()
            .trim()
            .min(3)
            .max(256)
            .optional()
            .describe("Stored Drive MIME type, including Google-native types for conversion."),
        })
        .refine((value) => value.textContent === undefined || value.base64Content === undefined, {
          message: "Provide textContent or base64Content, not both.",
        })
        .refine(
          (value) =>
            (value.textContent === undefined && value.base64Content === undefined) ||
            value.contentMimeType !== undefined,
          { message: "contentMimeType is required when content is provided." },
        ),
      annotations: mutating,
    },
    (input) => run(token, async () => {
      const title = nonBlank(input.title, "title", 1_024);
      const content = uploadContent(input.textContent, input.base64Content);
      if (content !== undefined && content.byteLength > MAX_UPLOAD_BYTES) {
        throw new DriveInputError(`Upload content exceeds ${MAX_UPLOAD_BYTES} bytes.`);
      }
      return drive.createFile({
        title,
        ...(input.parentId === undefined ? {} : { parentId: fileId(input.parentId) }),
        ...(input.contentMimeType === undefined
          ? {}
          : { contentMimeType: mimeType(input.contentMimeType) }),
        ...(input.driveMimeType === undefined
          ? {}
          : { driveMimeType: mimeType(input.driveMimeType) }),
        ...(content === undefined ? {} : { content }),
      });
    }),
  );

  server.registerTool(
    "copy_file",
    {
      description:
        "Copy an existing Drive file. Optionally give the copy a new title or parent folder. This creates one new file and never modifies the source.",
      inputSchema: z.object({
        fileId: fileIdSchema,
        title: z.string().min(1).max(1_024).optional(),
        parentId: fileIdSchema.optional(),
      }),
      annotations: mutating,
    },
    ({ fileId: id, title, parentId }) => run(token, () =>
      drive.copyFile(
        id,
        title === undefined ? undefined : nonBlank(title, "title", 1_024),
        parentId,
      )),
  );

  server.registerTool(
    "get_file_permissions",
    {
      description:
        "List people, groups, domains, and link permissions that can access one exact Drive file ID. This tool does not change sharing.",
      inputSchema: z.object({
        fileId: fileIdSchema,
        pageSize: z.number().int().min(1).max(100).default(100),
        pageToken: pageTokenSchema,
      }),
      annotations: readOnly,
    },
    ({ fileId: id, pageSize, pageToken }) =>
      run(token, () => drive.listPermissions(id, pageSize, pageToken)),
  );

  return server;
}

export function bearerToken(headers: Headers): string | undefined {
  const authorization = headers.get("authorization");
  if (authorization === null) return undefined;
  const match = /^Bearer ([^\s]+)$/i.exec(authorization);
  const token = match?.[1];
  return token !== undefined && token.length <= 16 * 1024 ? token : undefined;
}

function recentOrder(order: string): string {
  switch (order) {
    case "recency":
      return "recency desc";
    case "lastModified":
      return "modifiedTime desc";
    case "lastModifiedByMe":
      return "modifiedByMeTime desc";
    case "lastViewedByMe":
      return "viewedByMeTime desc";
    default:
      return "recency desc";
  }
}

function uploadContent(
  textContent: string | undefined,
  base64Content: string | undefined,
): Uint8Array | undefined {
  if (textContent !== undefined) {
    return new TextEncoder().encode(textContent);
  }
  return base64Content === undefined ? undefined : canonicalBase64(base64Content);
}

async function run<T extends JsonValue>(
  token: string,
  operation: () => Promise<T>,
): Promise<ToolResult> {
  try {
    return toolResult(await operation());
  } catch (error) {
    const failure = publicToolError(error, token);
    return { ...toolResult(failure), isError: true };
  }
}

function toolResult(value: JsonValue): ToolResult {
  const structuredContent = typeof value === "object" && value !== null && !Array.isArray(value)
    ? value
    : { value };
  return {
    content: [{ type: "text", text: JSON.stringify(structuredContent) }],
    structuredContent,
  };
}
