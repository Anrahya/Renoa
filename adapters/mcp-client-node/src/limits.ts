export const WIRE_VERSION = 6 as const;
export const MCP_PROTOCOL_VERSION = "2026-07-28";
export const MCP_ADAPTER_REVISION = "mcp-client-node-v0.6.0";

export const DISCOVERY_TIMEOUT_MS = 30_000;
export const TOOL_CALL_TIMEOUT_MS = 120_000;

export const MAX_STDIN_BYTES = 1 * 1024 * 1024;
export const MAX_HTTP_RESPONSE_BYTES = 16 * 1024 * 1024;
export const MAX_RECORD_BYTES = 20 * 1024 * 1024;
export const MAX_CATALOG_BYTES = 16 * 1024 * 1024;
export const MAX_STRUCTURED_CONTENT_BYTES = 4 * 1024 * 1024;
export const MAX_TOOL_RESULT_BYTES = 16 * 1024 * 1024;
export const MAX_TOOL_SCHEMA_BYTES = 1 * 1024 * 1024;
export const MAX_TOOL_DESCRIPTION_BYTES = 32 * 1024;
export const MAX_CURSOR_BYTES = 4 * 1024;
export const MAX_AUTH_TOKEN_BYTES = 16 * 1024;
export const MAX_OAUTH_STATE_BYTES = 512 * 1024;
export const MAX_OAUTH_VALUE_BYTES = 16 * 1024;
export const MAX_REQUEST_HEADERS = 64;
export const MAX_REQUEST_HEADER_BYTES = 32 * 1024;

export const MAX_DISCOVERY_PAGES = 64;
export const MAX_CATALOG_TOOLS = 1024;
export const MAX_CONTENT_BLOCKS = 256;
export const MAX_SCHEMA_NODES = 50_000;
export const MAX_SCHEMA_DEPTH = 128;

export const MAX_DIAGNOSTIC_BYTES = 4 * 1024;
export const MAX_FAILURE_MESSAGE_BYTES = 512;
