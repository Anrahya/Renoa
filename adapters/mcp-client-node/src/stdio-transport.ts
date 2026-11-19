import {
  StdioClientTransport,
  getDefaultEnvironment,
  type StdioServerParameters,
} from "@modelcontextprotocol/client/stdio";
import type { AdapterRequest, WireStdioTarget } from "./contract.js";
import { AdapterProblem } from "./errors.js";
import {
  MAX_HTTP_RESPONSE_BYTES,
  MAX_STDIO_ARG_BYTES,
  MAX_STDIO_ARGS,
  MAX_STDIO_COMMAND_BYTES,
  MAX_STDIO_CWD_BYTES,
  MAX_STDIO_ENV_KEY_BYTES,
  MAX_STDIO_ENV_VALUE_BYTES,
  MAX_STDIO_ENV_VARS,
  MAX_STDIO_STDERR_BYTES,
} from "./limits.js";
import type { CallExchangeTracker } from "./transport.js";
import { invalid, requireObject } from "./wire-values.js";

const STDIO_ENDPOINT = /^stdio:[a-f0-9]{64}$/;
// Beyond the SDK's default environment (HOME, PATH, USER and the like), a
// server inherits only where the person keeps its files and their locale.
// Anything a particular server needs comes from its own `env`.
const EXTRA_INHERITED_ENV_VARS = [
  "XDG_CONFIG_HOME",
  "XDG_DATA_HOME",
  "XDG_STATE_HOME",
  "XDG_CACHE_HOME",
  "LANG",
  "LC_ALL",
  "TZ",
] as const;

export function parseStdioEndpoint(raw: string): string {
  if (!STDIO_ENDPOINT.test(raw)) {
    throw new AdapterProblem(
      "invalid_request",
      "stdio request.endpoint must be a canonical stdio:<sha256> descriptor.",
      { code: "invalid_endpoint" },
    );
  }
  return raw;
}

export function parseOptionalStdio(
  value: unknown,
): { readonly stdio?: WireStdioTarget } {
  if (value === undefined) {
    return {};
  }
  const object = requireObject(value, "request.stdio");
  for (const key of Object.keys(object)) {
    if (key !== "command" && key !== "args" && key !== "env" && key !== "cwd") {
      throw invalid(`request.stdio contains unknown field '${key}'`);
    }
  }
  const command = requireCleanStdioString(
    object.command,
    "request.stdio.command",
    MAX_STDIO_COMMAND_BYTES,
    true,
  );
  let args: string[] | undefined;
  if (object.args !== undefined) {
    if (!Array.isArray(object.args)) {
      throw invalid("request.stdio.args must be an array");
    }
    if (object.args.length > MAX_STDIO_ARGS) {
      throw invalid(`request.stdio.args exceeds ${MAX_STDIO_ARGS} entries`);
    }
    args = object.args.map((item, index) =>
      requireCleanStdioString(
        item,
        `request.stdio.args[${index}]`,
        MAX_STDIO_ARG_BYTES,
        false,
      ),
    );
  }
  let env: Record<string, string> | undefined;
  if (object.env !== undefined) {
    const rawEnv = requireObject(object.env, "request.stdio.env");
    const entries = Object.entries(rawEnv);
    if (entries.length > MAX_STDIO_ENV_VARS) {
      throw invalid(`request.stdio.env exceeds ${MAX_STDIO_ENV_VARS} entries`);
    }
    env = {};
    for (const [key, val] of entries) {
      if (
        key.length === 0 ||
        Buffer.byteLength(key, "utf8") > MAX_STDIO_ENV_KEY_BYTES ||
        /[=\0\r\n]/u.test(key)
      ) {
        throw invalid(`request.stdio.env key '${key}' is invalid`);
      }
      env[key] = requireCleanStdioString(
        val,
        `request.stdio.env.${key}`,
        MAX_STDIO_ENV_VALUE_BYTES,
        false,
      );
    }
  }
  let cwd: string | undefined;
  if (object.cwd !== undefined) {
    cwd = requireCleanStdioString(
      object.cwd,
      "request.stdio.cwd",
      MAX_STDIO_CWD_BYTES,
      true,
    );
  }
  return {
    stdio: {
      command,
      ...(args === undefined ? {} : { args }),
      ...(env === undefined ? {} : { env }),
      ...(cwd === undefined ? {} : { cwd }),
    },
  };
}

export interface GuardedStdioTransport {
  readonly transport: StdioClientTransport;
  stderrSnippet(): string;
}

export function createGuardedStdioTransport(
  stdio: WireStdioTarget,
  action: Extract<AdapterRequest, { readonly action: "discover" | "call" }>["action"],
  tracker: CallExchangeTracker,
  dispatchStarted: () => Promise<void>,
  operationSignal: AbortSignal,
): GuardedStdioTransport {
  const params: StdioServerParameters = {
    command: stdio.command,
    ...(stdio.args === undefined ? {} : { args: [...stdio.args] }),
    env: buildStdioEnv(stdio.env),
    ...(stdio.cwd === undefined ? {} : { cwd: stdio.cwd }),
    stderr: "pipe",
    maxBufferSize: MAX_HTTP_RESPONSE_BYTES,
  };
  const transport = new StdioClientTransport(params);

  let stderrTail = "";
  const stderrStream = transport.stderr;
  if (stderrStream !== null) {
    stderrStream.on("data", (chunk: Buffer | string) => {
      const text = typeof chunk === "string" ? chunk : chunk.toString("utf8");
      stderrTail = (stderrTail + text).slice(-MAX_STDIO_STDERR_BYTES);
    });
  }

  // Keep transport as a base StdioClientTransport instance so SDK version
  // negotiation can use its disposable sibling probe when needed.
  let innerOnMessage = transport.onmessage;
  Object.defineProperty(transport, "onmessage", {
    configurable: true,
    enumerable: true,
    get() {
      return innerOnMessage;
    },
    set(handler: StdioClientTransport["onmessage"]) {
      innerOnMessage =
        handler === undefined
          ? undefined
          : (message) => {
              if (
                tracker.dispatchStarted &&
                "id" in message &&
                !("method" in message)
              ) {
                tracker.markResponseStarted();
              }
              handler(message);
            };
    },
  });

  let innerOnError = transport.onerror;
  Object.defineProperty(transport, "onerror", {
    configurable: true,
    enumerable: true,
    get() {
      return innerOnError;
    },
    set(handler: StdioClientTransport["onerror"]) {
      innerOnError =
        handler === undefined
          ? undefined
          : (error) => {
              if (
                error instanceof Error &&
                /ReadBuffer exceeded maximum size/i.test(error.message)
              ) {
                tracker.recordBoundaryProblem(
                  new AdapterProblem(
                    "resource_limit",
                    `MCP stdio response exceeds ${MAX_HTTP_RESPONSE_BYTES} bytes.`,
                    {
                      code: "stdio_response_limit",
                      partialChangesPossible: tracker.dispatchStarted,
                      cause: error,
                    },
                  ),
                );
              }
              handler(error);
            };
    },
  });

  const rawSend = transport.send.bind(transport);
  transport.send = async (message) => {
    const method =
      "method" in message && typeof message.method === "string"
        ? message.method
        : undefined;
    if (method !== undefined) {
      const allowed =
        method === "server/discover" ||
        method === "initialize" ||
        method === "notifications/initialized" ||
        method === "notifications/cancelled" ||
        (action === "discover" && method === "tools/list") ||
        (action === "call" && method === "tools/call");
      if (!allowed) {
        tracker.reject(
          new AdapterProblem(
            "protocol",
            `MCP adapter attempted unexpected stdio method '${method}'.`,
            {
              code: "unexpected_mcp_method",
              partialChangesPossible: tracker.dispatchStarted,
            },
          ),
        );
      }
      if (method === "tools/call") {
        if (tracker.callRequestCount !== 0) {
          tracker.reject(
            new AdapterProblem(
              "protocol",
              "MCP SDK attempted a hidden tools/call retry; Renoa blocked it.",
              { code: "hidden_retry_blocked", partialChangesPossible: true },
            ),
          );
        }
        if (operationSignal.aborted) {
          const abortError = new Error("MCP operation was cancelled before dispatch.");
          abortError.name = "AbortError";
          throw abortError;
        }
        await dispatchStarted();
        tracker.markDispatched();
      }
    }
    return rawSend(message);
  };

  return {
    transport,
    stderrSnippet(): string {
      return stderrTail.trim();
    },
  };
}

export function buildStdioEnv(
  customEnv?: Readonly<Record<string, string>>,
): Record<string, string> {
  const env: Record<string, string> = getDefaultEnvironment();
  for (const key of EXTRA_INHERITED_ENV_VARS) {
    const value = process.env[key];
    if (value !== undefined && !value.startsWith("()")) {
      env[key] = value;
    }
  }
  if (customEnv !== undefined) {
    for (const [key, value] of Object.entries(customEnv)) {
      env[key] = value;
    }
  }
  return env;
}

function requireCleanStdioString(
  value: unknown,
  label: string,
  maxBytes: number,
  nonEmptyNoNewlines: boolean,
): string {
  if (typeof value !== "string") {
    throw invalid(`${label} must be a string`);
  }
  if (nonEmptyNoNewlines && value.length === 0) {
    throw invalid(`${label} must be a non-empty string`);
  }
  if (Buffer.byteLength(value, "utf8") > maxBytes) {
    throw invalid(`${label} exceeds ${maxBytes} UTF-8 bytes`);
  }
  if (value.includes("\0") || (nonEmptyNoNewlines && /[\r\n]/u.test(value))) {
    throw invalid(`${label} contains forbidden control characters`);
  }
  return value;
}
