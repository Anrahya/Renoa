import assert from "node:assert/strict";
import test from "node:test";
import { OAuthExchangeTracker } from "../src/oauth-transport.js";

test("OAuth certainty tracks the credential POST, not earlier discovery responses", () => {
  const tracker = new OAuthExchangeTracker();
  tracker.markResponse("GET");
  tracker.markRequest("POST");
  assert.deepEqual(tracker.evidence(), {
    dispatchStarted: true,
    responseStarted: false,
  });
  tracker.markResponse("POST");
  assert.equal(tracker.evidence().responseStarted, true);
});
