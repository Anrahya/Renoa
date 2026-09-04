export const MCP_ORIGIN = "https://drive.renoa.live";
export const MCP_ENDPOINT = `${MCP_ORIGIN}/mcp`;
export const RESOURCE_METADATA_PATH = "/.well-known/oauth-protected-resource/mcp";

export const GOOGLE_ISSUER = "https://accounts.google.com";
export const GOOGLE_DRIVE_SCOPE = "https://www.googleapis.com/auth/drive";
export const GOOGLE_DRIVE_READONLY_SCOPE =
  "https://www.googleapis.com/auth/drive.readonly";
export const GOOGLE_DRIVE_FILE_SCOPE =
  "https://www.googleapis.com/auth/drive.file";

export const GOOGLE_API_ORIGIN = "https://www.googleapis.com";
export const GOOGLE_UPLOAD_ORIGIN = "https://www.googleapis.com";

export const FILE_FIELDS = [
  "id",
  "name",
  "mimeType",
  "size",
  "createdTime",
  "modifiedTime",
  "modifiedByMeTime",
  "viewedByMeTime",
  "webViewLink",
  "webContentLink",
  "parents",
  "shared",
  "trashed",
  "description",
  "driveId",
  "owners(displayName,emailAddress)",
  "capabilities(canEdit,canDownload,canShare,canCopy)",
  "shortcutDetails(targetId,targetMimeType)",
].join(",");

export const REQUEST_TIMEOUT_MS = 30_000;
export const MAX_JSON_RESPONSE_BYTES = 2 * 1024 * 1024;
export const MAX_READ_SOURCE_BYTES = 4 * 1024 * 1024;
export const DEFAULT_READ_CHARACTERS = 20_000;
export const MAX_READ_CHARACTERS = 40_000;
export const DEFAULT_DOWNLOAD_BYTES = 32 * 1024;
export const MAX_DOWNLOAD_BYTES = 256 * 1024;
export const MAX_UPLOAD_BYTES = 700 * 1024;
