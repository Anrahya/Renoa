import assert from "node:assert/strict";
import { test } from "node:test";

import { parseServerMessage } from "../src/protocol.js";

test("server numbers outside JavaScript's exact range are rejected", () => {
  assert.throws(
    () =>
      parseServerMessage(
        JSON.stringify({
          type: "task_list",
          request_id: Number.MAX_SAFE_INTEGER + 1,
          tasks: [],
        }),
      ),
    /request_id must be a safe unsigned integer/,
  );
});
