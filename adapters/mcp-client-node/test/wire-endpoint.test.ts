import assert from "node:assert/strict";
import test from "node:test";
import {
  ProtocolError,
  ProtocolErrorCode,
  SdkError,
  SdkErrorCode,
} from "@modelcontextprotocol/client";
import { parseEndpoint } from "../src/endpoint.js";
import { AdapterProblem, toWireFailure } from "../src/errors.js";
import { WIRE_VERSION } from "../src/limits.js";
import { parseAdapterRequest } from "../src/wire.js";

test("endpoint validation allows HTTPS and explicit loopback HTTP only", () => {
  assert.equal(
    parseEndpoint("https://example.com/mcp?public=1").href,
    "https://example.com/mcp?public=1",
  );
  assert.equal(parseEndpoint("http://127.1.2.3:8080/mcp").protocol, "http:");
  assert.equal(parseEndpoint("http://[::1]:8080/mcp").hostname, "[::1]");
  assert.throws(() => parseEndpoint("http://example.com/mcp"), AdapterProblem);
  assert.throws(
    () => parseEndpoint("https://user@example.com/mcp"),
    AdapterProblem,
  );
  assert.throws(
    () => parseEndpoint("https://example.com/mcp#fragment"),
    AdapterProblem,
  );
});

test("wire parser rejects unknown fields and non-object arguments", () => {
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint: "https://example.com/mcp",
        surprise: true,
      }),
    AdapterProblem,
  );
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "call",
        endpoint: "https://example.com/mcp",
        protocol_version: "2026-07-28",
        tool: { name: "x", input_schema: { type: "object" } },
        arguments: [],
      }),
    AdapterProblem,
  );
});

test("wire parser accepts bounded secret-backed credential headers", () => {
  const parsed = parseAdapterRequest({
    wire_version: WIRE_VERSION,
    action: "discover",
    endpoint: "https://example.com/mcp",
    credential: {
      scheme: "header",
      name: "X-API-Key",
      prefix: "",
      secret: "secret-token",
    },
  });
  assert.equal(parsed.action, "discover");
  if (parsed.action !== "discover") return;
  assert.deepEqual(parsed.credential, {
    scheme: "header",
    name: "x-api-key",
    prefix: "",
    secret: "secret-token",
  });
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint: "https://example.com/mcp",
        credential: {
          scheme: "header",
          name: "X-API-Key",
          prefix: "",
          secret: "bad\ntoken",
        },
      }),
    AdapterProblem,
  );
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint: "https://example.com/mcp",
        credential: {
          scheme: "header",
          name: "Content-Length",
          prefix: "",
          secret: "10",
        },
      }),
    AdapterProblem,
  );
});

test("wire parser normalizes bounded public headers and blocks client-owned fields", () => {
  const parsed = parseAdapterRequest({
    wire_version: WIRE_VERSION,
    action: "discover",
    endpoint: "https://example.com/mcp",
    headers: { "X-Exa-Source": "agent-plugin" },
  });
  assert.equal(parsed.action, "discover");
  if (parsed.action !== "discover") return;
  assert.deepEqual(parsed.headers, { "x-exa-source": "agent-plugin" });
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint: "https://example.com/mcp",
        headers: { authorization: "Bearer package-secret" },
      }),
    AdapterProblem,
  );
  assert.throws(
    () =>
      parseAdapterRequest({
        wire_version: WIRE_VERSION,
        action: "discover",
        endpoint: "https://example.com/mcp",
        headers: { Tenant: "one", tenant: "two" },
      }),
    AdapterProblem,
  );
});

test("diagnostics redact credentials before crossing the process boundary", () => {
  const failure = toWireFailure(
    new Error(
      'Authorization: Bearer secret-token client_secret="another-secret"',
    ),
  );
  assert.equal(failure.diagnostic.detail.includes("secret-token"), false);
  assert.equal(failure.diagnostic.detail.includes("another-secret"), false);
});

test("timeout certainty follows the dispatch boundary", () => {
  const timeout = new SdkError(SdkErrorCode.RequestTimeout, "timed out");
  const before = toWireFailure(timeout, {
    dispatchStarted: false,
    responseStarted: false,
  });
  const after = toWireFailure(timeout, {
    dispatchStarted: true,
    responseStarted: false,
  });
  assert.equal(before.kind, "timeout");
  assert.equal(before.certainty, "definite");
  assert.equal(after.kind, "timeout");
  assert.equal(after.certainty, "unknown");
});

test("a received protocol terminal wins a racing cancellation", () => {
  const rejection = new ProtocolError(
    ProtocolErrorCode.InternalError,
    "server rejected the tool call",
  );
  const failure = toWireFailure(
    rejection,
    { dispatchStarted: true, responseStarted: true },
    true,
  );
  assert.equal(failure.kind, "protocol");
  assert.equal(failure.certainty, "definite");
  assert.equal(failure.partial_changes_possible, true);
});
