import assert from "node:assert/strict";
import { test } from "node:test";

import { loadConfig } from "../src/config.js";

const requiredEnvironment = {
  RENOA_RCP_ENDPOINT: "wss://coordinator.example/connect",
  RENOA_RCP_DEVICE_ID: "00000000-0000-0000-0000-000000000001",
  RENOA_RCP_CREDENTIAL: "credential",
  RENOA_NODE_STATE: "/state/node.sqlite",
  RENOA_PI_INSTRUCTIONS: "Work carefully.",
  RENOA_PI_MODEL: "model",
  RENOA_PI_PROVIDER: "xai",
  RENOA_PI_AUTH_STORE: "/state/pi-auth.sqlite",
  RENOA_PI_TARGET: "workspace:renoa",
};

test("workspace configuration is optional", () => {
  assert.equal(loadConfig(requiredEnvironment).workspace, undefined);
});

test("a workspace root and access configure Pi's local tools", () => {
  assert.deepEqual(
    loadConfig({
      ...requiredEnvironment,
      RENOA_PI_WORKSPACE_ROOT: "/workspaces/renoa",
      RENOA_PI_WORKSPACE_ACCESS: "read_write",
    }),
    {
      endpoint: "wss://coordinator.example/connect",
      credentials: {
        deviceId: "00000000-0000-0000-0000-000000000001",
        credential: "credential",
      },
      statePath: "/state/node.sqlite",
      modelId: "model",
      provider: "xai",
      authStorePath: "/state/pi-auth.sqlite",
      instructions: "Work carefully.",
      target: "workspace:renoa",
      workspace: { root: "/workspaces/renoa", access: "read_write" },
    },
  );
});

test("provider and auth store configuration fail closed", () => {
  assert.throws(
    () => loadConfig({ ...requiredEnvironment, RENOA_PI_PROVIDER: "unknown" }),
    /RENOA_PI_PROVIDER must be opencode-go or xai/,
  );
  assert.throws(
    () => loadConfig({ ...requiredEnvironment, RENOA_PI_AUTH_STORE: "relative/auth.sqlite" }),
    /RENOA_PI_AUTH_STORE must be absolute/,
  );
});

test("workspace root and access must be configured together", () => {
  assert.throws(
    () =>
      loadConfig({
        ...requiredEnvironment,
        RENOA_PI_WORKSPACE_ROOT: "/workspaces/renoa",
      }),
    /RENOA_PI_WORKSPACE_ROOT and RENOA_PI_WORKSPACE_ACCESS must be set together/,
  );
  assert.throws(
    () =>
      loadConfig({
        ...requiredEnvironment,
        RENOA_PI_WORKSPACE_ACCESS: "read",
      }),
    /RENOA_PI_WORKSPACE_ROOT and RENOA_PI_WORKSPACE_ACCESS must be set together/,
  );
});

test("a workspace root must be absolute and access must be known", () => {
  assert.throws(
    () =>
      loadConfig({
        ...requiredEnvironment,
        RENOA_PI_WORKSPACE_ROOT: "relative/workspace",
        RENOA_PI_WORKSPACE_ACCESS: "read",
      }),
    /RENOA_PI_WORKSPACE_ROOT must be absolute/,
  );
  assert.throws(
    () =>
      loadConfig({
        ...requiredEnvironment,
        RENOA_PI_WORKSPACE_ROOT: "/workspaces/renoa",
        RENOA_PI_WORKSPACE_ACCESS: "admin",
      }),
    /RENOA_PI_WORKSPACE_ACCESS must be read or read_write/,
  );
});
