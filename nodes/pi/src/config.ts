import { isAbsolute } from "node:path";

import type { DeviceCredentials } from "./protocol.js";
import type { WorkspaceConfig } from "./workspace-tools.js";

export type PiProvider = "opencode-go" | "xai";

export interface NodeConfig {
  readonly endpoint: string;
  readonly credentials: DeviceCredentials;
  readonly statePath: string;
  readonly modelId: string;
  readonly provider: PiProvider;
  readonly authStorePath: string;
  readonly instructions: string;
  readonly target: string;
  readonly workspace?: WorkspaceConfig;
}

export function loadConfig(environment: NodeJS.ProcessEnv): NodeConfig {
  const workspaceConfig = workspace(environment);
  const provider = piProvider(environment);
  const authStorePath = loadAuthStorePath(environment);
  return {
    endpoint: required(environment, "RENOA_RCP_ENDPOINT"),
    credentials: {
      deviceId: required(environment, "RENOA_RCP_DEVICE_ID"),
      credential: required(environment, "RENOA_RCP_CREDENTIAL"),
    },
    statePath: required(environment, "RENOA_NODE_STATE"),
    modelId: required(environment, "RENOA_PI_MODEL"),
    provider,
    authStorePath,
    instructions: required(environment, "RENOA_PI_INSTRUCTIONS"),
    target: required(environment, "RENOA_PI_TARGET"),
    ...(workspaceConfig === undefined ? {} : { workspace: workspaceConfig }),
  };
}

export function loadAuthStorePath(environment: NodeJS.ProcessEnv): string {
  return absolute(environment, "RENOA_PI_AUTH_STORE");
}

function piProvider(environment: NodeJS.ProcessEnv): PiProvider {
  const provider = required(environment, "RENOA_PI_PROVIDER");
  if (provider !== "opencode-go" && provider !== "xai") {
    throw new Error("RENOA_PI_PROVIDER must be opencode-go or xai");
  }
  return provider;
}

function absolute(environment: NodeJS.ProcessEnv, name: string): string {
  const value = required(environment, name);
  if (!isAbsolute(value)) {
    throw new Error(`${name} must be absolute`);
  }
  return value;
}

function workspace(environment: NodeJS.ProcessEnv): WorkspaceConfig | undefined {
  const root = environment.RENOA_PI_WORKSPACE_ROOT || undefined;
  const access = environment.RENOA_PI_WORKSPACE_ACCESS || undefined;
  if (root === undefined && access === undefined) {
    return undefined;
  }
  if (root === undefined || access === undefined) {
    throw new Error(
      "RENOA_PI_WORKSPACE_ROOT and RENOA_PI_WORKSPACE_ACCESS must be set together",
    );
  }
  if (!isAbsolute(root)) {
    throw new Error("RENOA_PI_WORKSPACE_ROOT must be absolute");
  }
  if (access !== "read" && access !== "read_write") {
    throw new Error("RENOA_PI_WORKSPACE_ACCESS must be read or read_write");
  }
  return { root, access };
}

function required(environment: NodeJS.ProcessEnv, name: string): string {
  const value = environment[name];
  if (value === undefined || value === "") {
    throw new Error(`${name} is required`);
  }
  return value;
}
