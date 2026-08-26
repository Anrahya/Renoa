import assert from "node:assert/strict";
import test from "node:test";
import { AdapterProblem } from "../src/errors.js";
import { projectToolResult } from "../src/result.js";

test("tool errors remain completed model-visible content", () => {
  const result = projectToolResult(
    {
      content: [{ type: "text", text: "permission denied" }],
      isError: true,
    },
    false,
  );
  assert.equal(result.is_error, true);
  assert.deepEqual(result.content, [
    { type: "text", text: "permission denied" },
  ]);
});

test("unsupported mixed content fails atomically", () => {
  assert.throws(
    () =>
      projectToolResult(
        {
          content: [
            { type: "text", text: "do not leak this partial result" },
            { type: "audio", data: "YQ==", mimeType: "audio/wav" },
          ],
        },
        false,
      ),
    (error: unknown) =>
      error instanceof AdapterProblem &&
      error.code === "unsupported_content_type",
  );
});

test("invalid base64 and missing structured output fail after execution", () => {
  assert.throws(
    () =>
      projectToolResult(
        {
          content: [
            { type: "image", data: "not-base64", mimeType: "image/png" },
          ],
        },
        false,
      ),
    (error: unknown) =>
      error instanceof AdapterProblem && error.kind === "invalid_result",
  );
  assert.throws(
    () =>
      projectToolResult({ content: [{ type: "text", text: "done" }] }, true),
    (error: unknown) =>
      error instanceof AdapterProblem && error.kind === "invalid_result",
  );
});
