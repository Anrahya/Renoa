import { loadAuthStorePath } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";
import { fromOauth, xaiOAuth } from "./providers/xai.js";
import type { AuthEvent, AuthPrompt } from "./upstream/oauth-xai.js";

async function main(): Promise<void> {
  const credentials = new SqliteCredentialStore(loadAuthStorePath(process.env));
  const cancellation = new AbortController();
  const cancel = () => cancellation.abort();
  process.once("SIGINT", cancel);
  process.once("SIGTERM", cancel);
  try {
    const credential = await xaiOAuth.login({
      signal: cancellation.signal,
      prompt: unexpectedPrompt,
      notify: report,
    });
    credentials.write("xai", fromOauth(credential));
    process.stdout.write("SuperGrok login complete.\n");
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
      process.stdout.write(`Open ${event.verificationUri}\n`);
      process.stdout.write(`Enter code: ${event.userCode}\n`);
      break;
    case "auth_url":
      process.stdout.write(`Open ${event.url}\n`);
      if (event.instructions !== undefined) {
        process.stdout.write(`${event.instructions}\n`);
      }
      break;
    case "info":
    case "progress":
      process.stdout.write(`${event.message}\n`);
      break;
  }
}

main().catch((error: unknown) => {
  const failure = error instanceof Error ? error : new Error(String(error));
  process.stderr.write(`${failure.message}\n`);
  process.exitCode = 1;
});
