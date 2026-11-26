export const MCP_ORIGIN = "https://gmail.renoa.live";
export const MCP_ENDPOINT = `${MCP_ORIGIN}/mcp`;
export const RESOURCE_METADATA_PATH = "/.well-known/oauth-protected-resource/mcp";

export const GOOGLE_ISSUER = "https://accounts.google.com";
export const GOOGLE_GMAIL_MODIFY_SCOPE = "https://www.googleapis.com/auth/gmail.modify";
export const GOOGLE_GMAIL_READONLY_SCOPE = "https://www.googleapis.com/auth/gmail.readonly";
export const GOOGLE_GMAIL_COMPOSE_SCOPE = "https://www.googleapis.com/auth/gmail.compose";
export const GOOGLE_GMAIL_SEND_SCOPE = "https://www.googleapis.com/auth/gmail.send";

export const GOOGLE_GMAIL_ORIGIN = "https://gmail.googleapis.com";

export const REQUEST_TIMEOUT_MS = 30_000;
export const MAX_JSON_RESPONSE_BYTES = 4 * 1024 * 1024;
export const DEFAULT_READ_CHARACTERS = 20_000;
export const MAX_READ_CHARACTERS = 40_000;
export const MAX_BODY_BYTES = 200 * 1024;
export const MAX_HYDRATED_THREADS = 20;
