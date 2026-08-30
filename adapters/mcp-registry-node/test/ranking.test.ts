import assert from "node:assert/strict";
import test from "node:test";

import type { RegistryCandidate } from "../src/contract.js";
import {
  candidateMatchesQuery,
  compareCandidates,
  publisherNamespaceMatchesQuery,
} from "../src/ranking.js";

test("provider matching uses identity tokens instead of broad substrings", () => {
  const official = candidate("com.cloudflare.mcp/mcp", true);
  const unrelated = candidate("com.trycloudflare/tunnel", false);

  assert.equal(publisherNamespaceMatchesQuery("com.cloudflare.mcp", ["cloudflare"]), true);
  assert.equal(publisherNamespaceMatchesQuery("com.trycloudflare", ["cloudflare"]), false);
  assert.equal(candidateMatchesQuery(official, ["cloudflare"]), true);
  assert.equal(candidateMatchesQuery(unrelated, ["cloudflare"]), false);
  assert.ok(compareCandidates(official, unrelated, ["cloudflare"]) < 0);
});

function candidate(
  registryName: string,
  publisherMatch: boolean,
): RegistryCandidate {
  const namespace = registryName.split("/")[0] ?? registryName;
  return {
    registry_name: registryName,
    registry_version: "1.0.0",
    publisher_description: "fixture",
    publisher: { namespace, verification: "domain" },
    publisher_namespace_matches_query: publisherMatch,
    status: "active",
    remote_count: 1,
    streamable_http_count: 1,
    package_count: 0,
  };
}
