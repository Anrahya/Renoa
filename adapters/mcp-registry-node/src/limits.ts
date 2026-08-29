export const WIRE_VERSION = 1 as const;
export const ADAPTER_REVISION = "mcp-registry-node-v0.1.0";
export const OFFICIAL_REGISTRY_BASE_URL =
  "https://registry.modelcontextprotocol.io";

export const REQUEST_TIMEOUT_MS = 30_000;
export const MAX_STDIN_BYTES = 16 * 1024;
export const MAX_RECORD_BYTES = 512 * 1024;
export const MAX_RESPONSE_BYTES = 4 * 1024 * 1024;
export const MAX_QUERY_BYTES = 256;
export const MAX_QUERY_TOKENS = 6;
export const MAX_QUERY_VARIANTS = 8;
export const MAX_PAGE_RESULTS = 100;
export const MAX_PAGES_PER_QUERY = 3;
export const MAX_SEARCH_RESULTS = 100;
export const MAX_MODEL_RESULT_BYTES = 44 * 1024;
export const MAX_DESCRIPTION_BYTES = 1_024;
export const MAX_TITLE_BYTES = 512;
export const MAX_URL_BYTES = 8 * 1_024;
export const MAX_REMOTE_ENTRIES = 64;
export const MAX_PACKAGE_ENTRIES = 64;
export const MAX_INPUT_ENTRIES = 64;
export const MAX_DIAGNOSTIC_BYTES = 4 * 1_024;
export const MAX_FAILURE_MESSAGE_BYTES = 1_024;
