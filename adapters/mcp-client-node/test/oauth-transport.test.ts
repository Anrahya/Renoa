import assert from "node:assert/strict";
import test from "node:test";
import { once } from "node:events";
import { createServer } from "node:http";
import { AdapterProblem } from "../src/errors.js";
import { guardedOAuthFetch, OAuthExchangeTracker } from "../src/oauth-transport.js";

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

for (const method of ["GET", "POST"]) {
  test(`OAuth ${method} redirects remain blocked without forwarding requests`, async () => {
    let requests = 0;
    const server = createServer((_request, response) => {
      requests += 1;
      response.writeHead(307, { location: "/redirect-target" });
      response.end();
    });
    server.listen(0, "127.0.0.1");
    await once(server, "listening");
    try {
      const address = server.address();
      assert.ok(address !== null && typeof address === "object");
      const tracker = new OAuthExchangeTracker();
      const fetchFn = guardedOAuthFetch(tracker, new AbortController().signal);
      await assert.rejects(
        fetchFn(`http://127.0.0.1:${address.port}/oauth`, { method }),
        (error: unknown) => {
          assert.ok(error instanceof AdapterProblem);
          assert.equal(
            error.message,
            "OAuth endpoint redirect was blocked; metadata must name the final endpoint.",
          );
          return true;
        },
      );
      assert.equal(requests, 1);
      assert.deepEqual(tracker.evidence(), {
        dispatchStarted: method === "POST",
        responseStarted: method === "POST",
      });
    } finally {
      server.closeAllConnections();
      await new Promise<void>((resolve, reject) => {
        server.close((error) => error === undefined ? resolve() : reject(error));
      });
    }
  });
}
