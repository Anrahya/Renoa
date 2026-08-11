import assert from "node:assert/strict";
import { test } from "node:test";

import { Pulse } from "../src/pulse.js";

test("a wake between observing and waiting is not lost", async () => {
  const pulse = new Pulse();
  const observed = pulse.generation;

  pulse.wake();

  await pulse.wait(observed, AbortSignal.timeout(50));
  assert.equal(pulse.generation, observed + 1);
});
