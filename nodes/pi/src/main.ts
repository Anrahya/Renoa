import { loadConfig } from "./config.js";
import { PiHarness } from "./harness.js";
import { loadModelRuntime } from "./model-runtime.js";
import { PiNode } from "./node.js";

async function main(): Promise<void> {
  const config = loadConfig(process.env);
  const modelRuntime = await loadModelRuntime({
    provider: config.provider,
    modelId: config.modelId,
    authStorePath: config.authStorePath,
  });

  const shutdown = new AbortController();
  const stop = () => shutdown.abort();
  process.once("SIGINT", stop);
  process.once("SIGTERM", stop);
  try {
    const node = new PiNode({
      endpoint: config.endpoint,
      credentials: config.credentials,
      statePath: config.statePath,
      harness: new PiHarness({
        instructions: config.instructions,
        model: modelRuntime.model,
        streamFn: modelRuntime.streamFn,
        target: config.target,
        ...(config.workspace === undefined ? {} : { workspace: config.workspace }),
      }),
    });
    await node.run(shutdown.signal);
  } finally {
    process.removeListener("SIGINT", stop);
    process.removeListener("SIGTERM", stop);
    modelRuntime.close();
  }
}

main().catch((error: unknown) => {
  const failure = error instanceof Error ? error : new Error(String(error));
  console.error(failure.message);
  process.exitCode = 1;
});
