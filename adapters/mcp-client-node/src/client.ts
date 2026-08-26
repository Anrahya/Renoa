import {
  Client,
  StreamableHTTPClientTransport,
  type FetchLike,
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
} from "./contract.js";
import {
  AdapterProblem,
  type ExchangeEvidence,
  toWireFailure,
} from "./errors.js";
import { parseEndpoint } from "./endpoint.js";
import {
  DISCOVERY_TIMEOUT_MS,
  MAX_CATALOG_TOOLS,
  MAX_CURSOR_BYTES,
  MAX_DISCOVERY_PAGES,
  MAX_HTTP_RESPONSE_BYTES,
  MCP_ADAPTER_REVISION,
  MCP_PROTOCOL_VERSION,
  TOOL_CALL_TIMEOUT_MS,
  WIRE_VERSION,
} from "./limits.js";
import { projectToolResult } from "./result.js";

export interface AdapterHooks {
  readonly signal: AbortSignal;
  readonly dispatchStarted: () => Promise<void>;
  readonly registerCleanup: (cleanup: () => Promise<void>) => void;
}

export async function executeAdapterRequest(
  request: AdapterRequest,
  hooks: AdapterHooks,
): Promise<AdapterRecord> {
  const tracker = new CallExchangeTracker();
  try {
    const endpoint = parseEndpoint(request.endpoint);
    if (request.action === "discover") {
      const catalog = await discoverCatalog(endpoint, hooks, tracker);
      return { wire_version: WIRE_VERSION, event: "discovered", catalog };
    }
    const result = await callTool(endpoint, request, hooks, tracker);
    return { wire_version: WIRE_VERSION, event: "completed", result };
  } catch (error) {
    return {
      wire_version: WIRE_VERSION,
      event: "failed",
      failure: toWireFailure(
        tracker.boundaryProblem ?? error,
        tracker.evidence(),
        hooks.signal.aborted,
      ),
    };
  }
}

async function discoverCatalog(
  endpoint: URL,
  hooks: AdapterHooks,
  tracker: CallExchangeTracker,
) {
  const deadline = new Deadline(DISCOVERY_TIMEOUT_MS);
  const client = await connectClient(
    endpoint,
    "discover",
    deadline,
    hooks,
    tracker,
  );
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
      protocol_version: MCP_PROTOCOL_VERSION,
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
  action: AdapterRequest["action"],
  deadline: Deadline,
  hooks: AdapterHooks,
  tracker: CallExchangeTracker,
): Promise<Client> {
  const fetch = guardedFetch(
    action,
    tracker,
    hooks.dispatchStarted,
    hooks.signal,
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
  const client = new Client(
    { name: "renoa-mcp-client", version: "0.1.0" },
    {
      capabilities: {},
      supportedProtocolVersions: [MCP_PROTOCOL_VERSION],
      enforceStrictCapabilities: true,
      versionNegotiation: {
        mode: { pin: MCP_PROTOCOL_VERSION },
        probe: { maxRetries: 0 },
      },
      inputRequired: { autoFulfill: false },
      listMaxPages: MAX_DISCOVERY_PAGES,
    },
  );
  hooks.registerCleanup(() => client.close());
  await client.connect(transport, deadline.requestOptions(hooks.signal));
  if (client.getNegotiatedProtocolVersion() !== MCP_PROTOCOL_VERSION) {
    throw new AdapterProblem(
      "incompatible_protocol",
      `MCP endpoint did not negotiate ${MCP_PROTOCOL_VERSION}.`,
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

function guardedFetch(
  action: AdapterRequest["action"],
  tracker: CallExchangeTracker,
  dispatchStarted: () => Promise<void>,
  operationSignal: AbortSignal,
): FetchLike {
  const nativeFetch = globalThis.fetch;
  return async (input, init): Promise<Response> => {
    const requestMethod = (init?.method ?? "GET").toUpperCase();
    const mcpMethod = new Headers(init?.headers).get("mcp-method");
    if (requestMethod !== "POST" || mcpMethod === null) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP adapter attempted a transport operation outside stateless POST requests.",
          {
            code: "unexpected_transport_request",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }
    const allowed =
      mcpMethod === "server/discover" ||
      (action === "discover" && mcpMethod === "tools/list") ||
      (action === "call" && mcpMethod === "tools/call");
    if (!allowed) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          `MCP adapter attempted unexpected method '${mcpMethod}'.`,
          {
            code: "unexpected_mcp_method",
            partialChangesPossible: tracker.dispatchStarted,
          },
        ),
      );
    }

    const isToolCall = mcpMethod === "tools/call";
    if (isToolCall) {
      if (tracker.callRequestCount !== 0) {
        tracker.reject(
          new AdapterProblem(
            "protocol",
            "MCP SDK attempted a hidden tools/call retry; Renoa blocked it.",
            {
              code: "hidden_retry_blocked",
              partialChangesPossible: true,
            },
          ),
        );
      }
      await dispatchStarted();
      tracker.markDispatched();
    }

    const signal =
      init?.signal == null
        ? operationSignal
        : AbortSignal.any([operationSignal, init.signal]);
    const response = await nativeFetch(input, {
      ...init,
      redirect: "manual",
      signal,
    });
    if (isToolCall) {
      tracker.markResponseStarted();
    }
    if (response.headers.has("mcp-session-id")) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP endpoint attempted to create a session; Renoa v0 is stateless.",
          {
            code: "session_not_supported",
            partialChangesPossible: isToolCall,
            httpStatus: response.status,
          },
        ),
      );
    }
    return boundResponse(response, isToolCall, tracker);
  };
}

function boundResponse(
  response: Response,
  toolCall: boolean,
  tracker: CallExchangeTracker,
): Response {
  const contentLength = response.headers.get("content-length");
  if (contentLength !== null) {
    const parsed = Number(contentLength);
    if (!Number.isSafeInteger(parsed) || parsed < 0) {
      tracker.reject(
        new AdapterProblem(
          "protocol",
          "MCP response has an invalid Content-Length.",
          {
            code: "invalid_content_length",
            partialChangesPossible: toolCall,
            httpStatus: response.status,
          },
        ),
      );
    }
    if (parsed > MAX_HTTP_RESPONSE_BYTES) {
      tracker.reject(responseLimit(toolCall, response.status));
    }
  }
  if (response.body === null) {
    return response;
  }

  let bytes = 0;
  const limiter = new TransformStream<Uint8Array, Uint8Array>({
    transform(chunk, controller) {
      bytes += chunk.byteLength;
      if (bytes > MAX_HTTP_RESPONSE_BYTES) {
        const problem = responseLimit(toolCall, response.status);
        tracker.recordBoundaryProblem(problem);
        throw problem;
      }
      controller.enqueue(chunk);
    },
  });
  return new Response(response.body.pipeThrough(limiter), {
    status: response.status,
    statusText: response.statusText,
    headers: response.headers,
  });
}

function responseLimit(toolCall: boolean, status: number): AdapterProblem {
  return new AdapterProblem(
    "resource_limit",
    `MCP response exceeds ${MAX_HTTP_RESPONSE_BYTES} bytes.`,
    {
      code: "http_response_limit",
      partialChangesPossible: toolCall,
      httpStatus: status,
    },
  );
}

class CallExchangeTracker {
  callRequestCount = 0;
  dispatchStarted = false;
  responseStarted = false;
  boundaryProblem: AdapterProblem | undefined;

  markDispatched(): void {
    this.callRequestCount += 1;
    this.dispatchStarted = true;
  }

  markResponseStarted(): void {
    this.responseStarted = true;
  }

  recordBoundaryProblem(problem: AdapterProblem): void {
    this.boundaryProblem ??= problem;
  }

  reject(problem: AdapterProblem): never {
    this.recordBoundaryProblem(problem);
    throw problem;
  }

  evidence(): ExchangeEvidence {
    return {
      dispatchStarted: this.dispatchStarted,
      responseStarted: this.responseStarted,
    };
  }
}

class Deadline {
  readonly expiresAt: number;

  constructor(durationMs: number) {
    this.expiresAt = Date.now() + durationMs;
  }

  requestOptions(signal: AbortSignal) {
    const remaining = this.expiresAt - Date.now();
    if (remaining <= 0) {
      throw new AdapterProblem(
        "timeout",
        "MCP operation exceeded its total deadline.",
        {
          code: "total_deadline",
        },
      );
    }
    return {
      signal,
      timeout: remaining,
      maxTotalTimeout: remaining,
      resetTimeoutOnProgress: false,
    } as const;
  }
}
