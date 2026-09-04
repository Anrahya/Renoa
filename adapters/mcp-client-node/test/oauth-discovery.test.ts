import assert from "node:assert/strict";
import test from "node:test";
import { WIRE_VERSION } from "../src/limits.js";
import { OAuthFixture } from "./oauth-fixture.js";
import { runAdapter } from "./support.js";

test("OAuth preflight discovers one issuer and its supported client modes without a POST", async () => {
  const server = new OAuthFixture({ clientMetadataSupported: true });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "oauth_discovered", JSON.stringify(record));
    if (record?.event !== "oauth_discovered") return;
    assert.deepEqual(record.discovery, {
      mcp_endpoint: server.endpoint,
      issuer: server.origin,
      client_id_metadata_document_supported: true,
      dynamic_client_registration_supported: true,
    });
    assert.equal(server.registrationRequests, 0);
    assert.equal(server.tokenRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight fails closed when protected-resource metadata is absent", async () => {
  const server = new OAuthFixture({ omitResourceMetadata: true });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(
      record.failure.diagnostic.code,
      "oauth_resource_metadata_missing",
    );
    assert.equal(record.failure.partial_changes_possible, false);
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight rejects ambiguous authorization servers", async () => {
  const server = new OAuthFixture({
    authorizationServers: [
      "https://accounts-one.example",
      "https://accounts-two.example",
    ],
  });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(
      record.failure.diagnostic.code,
      "oauth_authorization_server_ambiguous",
    );
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight rejects a resource with no authorization server", async () => {
  const server = new OAuthFixture({ authorizationServers: [] });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(
      record.failure.diagnostic.code,
      "oauth_authorization_server_missing",
    );
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight rejects protected-resource metadata without an authorization server", async () => {
  const server = new OAuthFixture({ omitAuthorizationServers: true });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(
      record.failure.diagnostic.code,
      "oauth_authorization_server_missing",
    );
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight rejects metadata for a different protected resource", async () => {
  const server = new OAuthFixture({
    resource: "https://different.example/mcp",
  });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(record.failure.diagnostic.code, "oauth_resource_mismatch");
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});

test("OAuth preflight rejects an authorization server without metadata", async () => {
  const server = new OAuthFixture({ omitAuthorizationMetadata: true });
  await server.start();
  try {
    const result = await runAdapter({
      wire_version: WIRE_VERSION,
      action: "oauth_discover",
      endpoint: server.endpoint,
    });
    const record = result.records[0];
    assert.equal(record?.event, "failed", JSON.stringify(record));
    if (record?.event !== "failed") return;
    assert.equal(
      record.failure.diagnostic.code,
      "oauth_authorization_metadata_missing",
    );
    assert.equal(server.registrationRequests, 0);
  } finally {
    await server.close();
  }
});
