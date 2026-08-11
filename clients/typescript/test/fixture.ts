import { spawn, type ChildProcess } from "node:child_process";
import { mkdtemp, rm } from "node:fs/promises";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import { createInterface } from "node:readline";

import type { DeviceCredentials } from "../src/index.js";

export interface Fixture {
  readonly endpoint: string;
  readonly lossyEndpoint: string;
  readonly replayEndpoint: string;
  readonly credentials: DeviceCredentials;
  readonly unownedTaskId: string;
  readonly statePath: string;
  readonly tasks: readonly [
    { readonly taskId: string; readonly target: string },
    { readonly taskId: string; readonly target: string },
    { readonly taskId: string; readonly target: string },
  ];
  stop(): Promise<void>;
}

type FixtureDescription = Omit<Fixture, "statePath" | "stop">;

export async function startFixture(): Promise<Fixture> {
  const directory = await mkdtemp(join(tmpdir(), "renoa-typescript-"));
  const repository = resolve(import.meta.dirname, "../../..");
  const process = spawn(
    "cargo",
    [
      "run",
      "--quiet",
      "-p",
      "renoa-control",
      "--example",
      "typescript_surface_fixture",
      "--",
      join(directory, "control.sqlite"),
    ],
    {
      cwd: repository,
      stdio: ["ignore", "pipe", "inherit"],
    },
  );

  try {
    const description = await readDescription(process);
    return {
      ...description,
      statePath: join(directory, "surface.sqlite"),
      async stop() {
        await stopProcess(process);
        await rm(directory, { force: true, recursive: true });
      },
    };
  } catch (error) {
    await stopProcess(process);
    await rm(directory, { force: true, recursive: true });
    throw error;
  }
}

async function readDescription(process: ChildProcess): Promise<FixtureDescription> {
  if (process.stdout === null) {
    throw new Error("fixture stdout is unavailable");
  }
  const lines = createInterface({ input: process.stdout });
  const exited = new Promise<never>((_resolve, reject) => {
    process.once("exit", (code, signal) => {
      reject(new Error(`fixture exited before startup (${code ?? signal})`));
    });
  });
  const line = await Promise.race([
    (async () => {
      for await (const value of lines) {
        return value;
      }
      throw new Error("fixture closed stdout before startup");
    })(),
    exited,
  ]);
  lines.close();
  return JSON.parse(line) as FixtureDescription;
}

async function stopProcess(process: ChildProcess): Promise<void> {
  const exited = new Promise<void>((resolveExit) => {
    process.once("exit", () => resolveExit());
  });
  if (process.exitCode !== null || process.signalCode !== null) {
    return;
  }
  process.kill("SIGTERM");
  await exited;
}
