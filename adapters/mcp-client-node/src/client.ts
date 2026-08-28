import {
  Client,
  SUPPORTED_PROTOCOL_VERSIONS as LEGACY_PROTOCOL_VERSIONS,
  StreamableHTTPClientTransport,
  type ListToolsResult,
  type Tool,
} from "@modelcontextprotocol/client";
import {
  finalizeCatalog,
  inspectDiscoveredTool,
  validateFrozenTool,
} from "./catalog.js";
import type {
  AdapterRecord,
  AdapterRequest,
  CatalogTool,
  RejectedTool,
  WireAuthorization,
} from "./contract.js";
import {
  AdapterProblem,
  toWireFailure,
} from "./errors.js";
import { parseEndpoint } from "./endpoint.js";
import {
  DISCOVERY_TIMEOUT_MS,
  MAX_CATALOG_TOOLS,
  MAX_CURSOR_BYTES,
  MAX_DISCOVERY_PAGES,
  MCP_ADAPTER_REVISION,
  MCP_PROTOCOL_VERSION,
  TOOL_CALL_TIMEOUT_MS,
  WIRE_VERSION,
} from "./limits.js";
import { projectToolResult } from "./result.js";
import { executeOAuthRequest, isOAuthRequest } from "./oauth.js";
import {
  CallExchangeTracker,
  Deadline,
  guardedFetch,
} from "./transport.js";

type RuntimeRequest = Extract<
  AdapterRequest,
  { readonly action: "discover" | "call" }
>;

export interface AdapterHooks {
  readonly signal: AbortSignal;
  readonly dispatchStarted: () => Promise<void>;
  readonly registerCleanup: (cleanup: () => Promise<void>) => void;
}

export async function executeAdapterRequest(
  request: AdapterRequest,
  hooks: AdapterHooks,
): Promise<AdapterRecord> {
  if (isOAuthRequest(request)) {
    return executeOAuthRequest(request, hooks.signal);
  }
  const tracker = new CallExchangeTracker();
  let record: AdapterRecord;
  try {
    const endpoint = parseEndpoint(request.endpoint);
    if (request.action === "discover") {
      const catalog = await discoverCatalog(
        endpoint,
        hooks,
        tracker,
        request.headers,
        request.authorization,
      );
      record = { wire_version: WIRE_VERSION, event: "discovered", catalog };
    } else {
      const result = await callTool(endpoint, request, hooks, tracker);
      record = { wire_version: WIRE_VERSION, event: "completed", result };
    }
  } catch (error) {
    record = {
      wire_version: WIRE_VERSION,
      event: "failed",
      failure: toWireFailure(
        tracker.boundaryProblem ?? error,
        tracker.evidence(),
        hooks.signal.aborted,
      ),
    };
  }
  return redactCredential(record, request.authorization?.token);
}

function redactCredential(
  record: AdapterRecord,
  token: string | undefined,
): AdapterRecord {
  if (token === undefined) {
    return record;
  }
  const pending: unknown[] = [record];
  while (pending.length > 0) {
    const current = pending.pop();
    if (Array.isArray(current)) {
      pending.push(...current);
      continue;
    }
    if (typeof current !== "object" || current === null) {
      continue;
    }
    for (const [key, value] of Object.entries(current)) {
      if (typeof value === "string" && value.includes(token)) {
        (current as Record<string, unknown>)[key] = value.replaceAll(
          token,
          "[REDACTED]",
        );
      } else if (typeof value === "object" && value !== null) {
        pending.push(value);
      }
    }
  }
  return record;
}

async function discoverCatalog(
  endpoint: URL,
  hooks: AdapterHooks,
  tracker: CallExchangeTracker,
  headers: RuntimeRequest["headers"],
  authorization: WireAuthorization | undefined,
) {
  const deadline = new Deadline(DISCOVERY_TIMEOUT_MS);
  const client = await connectClient(
    endpoint,
    "discover",
    deadline,
    hooks,
    tracker,
    headers,
    authorization,
    undefined,
  );
  const protocolVersion = requireNegotiatedProtocolVersion(client);
  const accepted: CatalogTool[] = [];
  const rejected: RejectedTool[] = [];
  const seenCursors = new Set<string>();
  let cursor: string | undefined;
  let pages = 0;
  let rawToolCount = 0;

  do {
    if (pages >= MAX_DISCOVERY_PAGES) {
      throw new AdapterProblem(
        "resource_limit",
        `MCP tools/list exceeded ${MAX_DISCOVERY_PAGES} pages.`,
        { code: "pagination_limit" },
      );
    }
    const params = cursor === undefined ? {} : { cursor };
    const page: ListToolsResult = await client.request(
      { method: "tools/list", params },
      deadline.requestOptions(hooks.signal),
    );
    pages += 1;

    if (rawToolCount + page.tools.length > MAX_CATALOG_TOOLS) {
      throw new AdapterProblem(
        "resource_limit",
        `MCP catalog exceeds ${MAX_CATALOG_TOOLS} tools.`,
        { code: "tool_count_limit" },
      );
    }
    for (const tool of page.tools) {
      const inspected = inspectDiscoveredTool(tool, rawToolCount);
      if ("accepted" in inspected) {
        accepted.push(inspected.accepted);
      } else {
        rejected.push(inspected.rejected);
      }
      rawToolCount += 1;
    }

    const nextCursor = page.nextCursor;
    if (nextCursor !== undefined) {
      if (Buffer.byteLength(nextCursor, "utf8") > MAX_CURSOR_BYTES) {
        throw new AdapterProblem(
          "resource_limit",
          `MCP pagination cursor exceeds ${MAX_CURSOR_BYTES} bytes.`,
          { code: "cursor_limit" },
        );
      }
      if (seenCursors.has(nextCursor)) {
        throw new AdapterProblem(
          "protocol",
          "MCP tools/list repeated a pagination cursor.",
          { code: "pagination_cycle" },
        );
      }
      seenCursors.add(nextCursor);
    }
    cursor = nextCursor;
  } while (cursor !== undefined);

  return finalizeCatalog(
    {
      endpoint: endpoint.href,
      protocol_version: protocolVersion,
      adapter_revision: MCP_ADAPTER_REVISION,
    },
    accepted,
    rejected,
  );
}

async function callTool(
  endpoint: URL,
  request: Extract<AdapterRequest, { readonly action: "call" }>,
  hooks: AdapterHooks,
  tracker: CallExchangeTracker,
) {
  const selected = validateFrozenTool(request.tool);
  const deadline = new Deadline(TOOL_CALL_TIMEOUT_MS);
  const client = await connectClient(
    endpoint,
    "call",
    deadline,
    hooks,
    tracker,
    request.headers,
    request.authorization,
    request.protocol_version,
  );
  const toolDefinition: Tool = {
    name: selected.name,
    inputSchema: selected.input_schema as Tool["inputSchema"],
    ...(selected.output_schema === undefined
      ? {}
      : { outputSchema: selected.output_schema }),
  };
  const result = (await client.callTool(
    { name: selected.name, arguments: request.arguments },
    {
      ...deadline.requestOptions(hooks.signal),
      allowInputRequired: true,
      toolDefinition,
    },
  )) as unknown;
  if (tracker.callRequestCount !== 1) {
    throw new AdapterProblem(
      "internal",
      "MCP adapter completed without exactly one tools/call request.",
      {
        code: "invalid_dispatch_count",
        partialChangesPossible: tracker.callRequestCount > 0,
      },
    );
  }
  return projectToolResult(result, selected.output_schema !== undefined);
}

async function connectClient(
  endpoint: URL,
  action: RuntimeRequest["action"],
  deadline: Deadline,
  hooks: AdapterHooks,
  tracker: CallExchangeTracker,
  headers: RuntimeRequest["headers"],
  authorization: WireAuthorization | undefined,
  expectedProtocolVersion: string | undefined,
): Promise<Client> {
  const fetch = guardedFetch(
    action,
    tracker,
    hooks.dispatchStarted,
    hooks.signal,
    endpoint,
    headers,
    authorization,
  );
  const transport = new StreamableHTTPClientTransport(endpoint, {
    fetch,
    requestInit: { redirect: "manual" },
    reconnectionOptions: {
      maxReconnectionDelay: 1,
      initialReconnectionDelay: 1,
      reconnectionDelayGrowFactor: 1,
      maxRetries: 0,
    },
    onInsufficientScope: "throw",
    maxStepUpRetries: 0,
  });
  const protocol = protocolSelection(expectedProtocolVersion);
  const client = new Client(
    { name: "renoa-mcp-client", version: "0.1.0" },
    {
      capabilities: {},
      supportedProtocolVersions: protocol.supportedVersions,
      enforceStrictCapabilities: true,
      versionNegotiation: {
        mode: protocol.mode,
        probe: { maxRetries: 0 },
      },
      inputRequired: { autoFulfill: false },
      listMaxPages: MAX_DISCOVERY_PAGES,
    },
  );
  hooks.registerCleanup(() => client.close());
  await client.connect(transport, deadline.requestOptions(hooks.signal));
  const negotiated = requireNegotiatedProtocolVersion(client);
  if (
    expectedProtocolVersion !== undefined &&
    negotiated !== expectedProtocolVersion
  ) {
    throw new AdapterProblem(
      "incompatible_protocol",
      `MCP endpoint negotiated ${negotiated}, expected ${expectedProtocolVersion}.`,
      { code: "protocol_version_mismatch" },
    );
  }
  if (client.getServerCapabilities()?.tools === undefined) {
    throw new AdapterProblem(
      "incompatible_protocol",
      "MCP endpoint does not advertise the tools capability.",
      { code: "tools_capability_missing" },
    );
  }
  return client;
}

const SUPPORTED_PROTOCOL_VERSIONS = [
  MCP_PROTOCOL_VERSION,
  ...LEGACY_PROTOCOL_VERSIONS,
] as const;
const SUPPORTED_PROTOCOL_VERSION_SET = new Set<string>(
  SUPPORTED_PROTOCOL_VERSIONS,
);

function protocolSelection(expected: string | undefined): {
  readonly supportedVersions: string[];
  readonly mode: "auto" | "legacy" | { readonly pin: string };
} {
  if (expected === undefined) {
    return {
      supportedVersions: [...SUPPORTED_PROTOCOL_VERSIONS],
      mode: "auto",
    };
  }
  if (!SUPPORTED_PROTOCOL_VERSION_SET.has(expected)) {
    throw new AdapterProblem(
      "incompatible_protocol",
      `Renoa's MCP adapter does not support catalog protocol ${expected}.`,
      { code: "unsupported_catalog_protocol" },
    );
  }
  return {
    supportedVersions: [expected],
    mode: expected === MCP_PROTOCOL_VERSION ? { pin: expected } : "legacy",
  };
}

function requireNegotiatedProtocolVersion(client: Client): string {
  const negotiated = client.getNegotiatedProtocolVersion();
  if (
    negotiated === undefined ||
    !SUPPORTED_PROTOCOL_VERSION_SET.has(negotiated)
  ) {
    throw new AdapterProblem(
      "incompatible_protocol",
      "The MCP endpoint negotiated an unsupported protocol version.",
      { code: "unsupported_negotiated_protocol" },
    );
  }
  return negotiated;
}
