import { createModels, type AuthEvent, type AuthPrompt } from "@earendil-works/pi-ai";
import { xaiProvider } from "@earendil-works/pi-ai/providers/xai";

import { loadAuthStorePath } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";

async function main(): Promise<void> {
  const credentials = new SqliteCredentialStore(loadAuthStorePath(process.env));
  const models = createModels({ credentials });
  models.setProvider(xaiProvider());
  const cancellation = new AbortController();
  const cancel = () => cancellation.abort();
  process.once("SIGINT", cancel);
  process.once("SIGTERM", cancel);
  try {
    await models.login("xai", "oauth", {
      signal: cancellation.signal,
      prompt: unexpectedPrompt,
      notify: report,
    });
    console.log("SuperGrok login complete.");
  } finally {
    process.removeListener("SIGINT", cancel);
    process.removeListener("SIGTERM", cancel);
    credentials.close();
  }
}

function unexpectedPrompt(prompt: AuthPrompt): Promise<string> {
  return Promise.reject(new Error(`xAI OAuth requested unexpected input: ${prompt.message}`));
}

function report(event: AuthEvent): void {
  switch (event.type) {
    case "device_code":
      console.log(`Open ${event.verificationUri}`);
      console.log(`Enter code: ${event.userCode}`);
      break;
    case "auth_url":
      console.log(`Open ${event.url}`);
      if (event.instructions !== undefined) {
        console.log(event.instructions);
      }
      break;
    case "info":
      console.log(event.message);
      break;
    case "progress":
      console.log(event.message);
      break;
  }
}

main().catch((error: unknown) => {
  const failure = error instanceof Error ? error : new Error(String(error));
  console.error(failure.message);
  process.exitCode = 1;
});
