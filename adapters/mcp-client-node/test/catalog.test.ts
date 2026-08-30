import assert from "node:assert/strict";
import test from "node:test";
import type { Tool } from "@modelcontextprotocol/client";
import { inspectDiscoveredTool } from "../src/catalog.js";

test("nested reachable headers stay executable but leave the model schema", () => {
  const inspected = inspectDiscoveredTool(
    tool({
      type: "object",
      properties: {
        routing: {
          type: "object",
          properties: {
            tenant: { type: "string", "x-mcp-header": "Tenant" },
          },
        },
      },
    }),
    0,
  );
  assert.equal("accepted" in inspected, true);
  if (!("accepted" in inspected)) return;
  const rawRouting = inspected.accepted.input_schema.properties as Record<
    string,
    Record<string, unknown>
  >;
  const modelRouting = inspected.accepted.model_input_schema
    .properties as Record<string, Record<string, unknown>>;
  const rawTenant = (rawRouting.routing?.properties as Record<string, unknown>)
    .tenant as Record<string, unknown>;
  const modelTenant = (
    modelRouting.routing?.properties as Record<string, unknown>
  ).tenant as Record<string, unknown>;
  assert.equal(rawTenant["x-mcp-header"], "Tenant");
  assert.equal("x-mcp-header" in modelTenant, false);
});

test("invalid header placement, type, and duplicate names isolate one tool", () => {
  const cases = [
    {
      type: "object",
      oneOf: [
        {
          type: "object",
          properties: {
            tenant: { type: "string", "x-mcp-header": "Tenant" },
          },
        },
      ],
    },
    {
      type: "object",
      properties: {
        ratio: { type: "number", "x-mcp-header": "Ratio" },
      },
    },
    {
      type: "object",
      properties: {
        left: { type: "string", "x-mcp-header": "Tenant" },
        right: { type: "string", "x-mcp-header": "TENANT" },
      },
    },
  ] as const;
  for (const inputSchema of cases) {
    const inspected = inspectDiscoveredTool(tool(inputSchema), 3);
    assert.equal("rejected" in inspected, true);
    if ("rejected" in inspected) {
      assert.equal(inspected.rejected.index, 3);
      assert.match(inspected.rejected.reason, /x-mcp-header/);
    }
  }
});

test("over-deep schemas are rejected at the catalog boundary", () => {
  const inputSchema: Record<string, unknown> = {
    type: "object",
    properties: {},
  };
  let current = inputSchema;
  for (let depth = 0; depth < 130; depth += 1) {
    const child: Record<string, unknown> = { type: "object", properties: {} };
    current.properties = { child };
    current = child;
  }

  const inspected = inspectDiscoveredTool(
    tool(inputSchema as Tool["inputSchema"]),
    4,
  );
  assert.equal("rejected" in inspected, true);
  if ("rejected" in inspected) {
    assert.equal(inspected.rejected.index, 4);
    assert.match(inspected.rejected.reason, /nesting depth/);
  }
});

test("an input schema that cannot compile is isolated during discovery", () => {
  const inspected = inspectDiscoveredTool(
    tool({
      type: "object",
      properties: { value: { type: "string", pattern: "[" } },
    }),
    7,
  );
  assert.equal("rejected" in inspected, true);
  if ("rejected" in inspected) {
    assert.equal(inspected.rejected.index, 7);
    assert.match(inspected.rejected.reason, /cannot be compiled/u);
  }
});

test("reused schema ids cannot substitute an earlier tool schema", () => {
  const first = inspectDiscoveredTool(
    tool({
      $id: "https://example.com/shared-schema",
      type: "object",
      properties: { value: { type: "string" } },
    }),
    1,
  );
  const second = inspectDiscoveredTool(
    tool({
      $id: "https://example.com/shared-schema",
      type: "object",
      properties: { value: { type: "string", pattern: "[" } },
    }),
    2,
  );

  assert.equal("accepted" in first, true);
  assert.equal("rejected" in second, true);
});

test("one tool schema cannot resolve references through another tool", () => {
  const first = inspectDiscoveredTool(
    tool({
      $id: "https://example.com/first-schema",
      type: "object",
      $defs: { value: { type: "string" } },
    }),
    1,
  );
  const second = inspectDiscoveredTool(
    tool({
      type: "object",
      properties: {
        value: { $ref: "https://example.com/first-schema#/$defs/value" },
      },
    }),
    2,
  );

  assert.equal("accepted" in first, true);
  assert.equal("rejected" in second, true);
});

function tool(inputSchema: Tool["inputSchema"]): Tool {
  return { name: "example", inputSchema };
}
