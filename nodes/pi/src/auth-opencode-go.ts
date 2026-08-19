import { createModels, type AuthPrompt } from "@earendil-works/pi-ai";
import { opencodeGoProvider } from "@earendil-works/pi-ai/providers/opencode-go";

import { loadAuthStorePath } from "./config.js";
import { SqliteCredentialStore } from "./credentials.js";

const MAX_API_KEY_BYTES = 4_096;

async function main(): Promise<void> {
  if (process.stdin.isTTY) {
    throw new Error(
      "OpenCode Go authentication requires an API key piped on standard input; see nodes/pi/README.md",
    );
  }
  process.stdin.setEncoding("utf8");
  const apiKey = await readApiKey(process.stdin);
  const credentials = new SqliteCredentialStore(loadAuthStorePath(process.env));
  const models = createModels({ credentials });
  models.setProvider(opencodeGoProvider());
  let supplied = false;
  try {
    const credential = await models.login("opencode-go", "api_key", {
      prompt: (prompt) => {
        if (supplied || prompt.type !== "secret") {
          return unexpectedPrompt(prompt);
        }
        supplied = true;
        return Promise.resolve(apiKey);
      },
      notify: () => {},
    });
    if (!supplied || credential.type !== "api_key" || credential.key !== apiKey) {
      throw new Error("OpenCode Go returned an invalid API-key credential");
    }
    console.log("OpenCode Go API key stored.");
  } finally {
    credentials.close();
  }
}

async function readApiKey(input: AsyncIterable<unknown>): Promise<string> {
  let encoded = "";
  let bytes = 0;
  for await (const chunk of input) {
    if (typeof chunk !== "string") {
      throw new Error("OpenCode Go API key input is not UTF-8 text");
    }
    bytes += Buffer.byteLength(chunk);
    if (bytes > MAX_API_KEY_BYTES + 2) {
      throw new Error(`OpenCode Go API key exceeds ${MAX_API_KEY_BYTES} bytes`);
    }
    encoded += chunk;
  }
  const apiKey = encoded.endsWith("\r\n")
    ? encoded.slice(0, -2)
    : encoded.endsWith("\n")
      ? encoded.slice(0, -1)
      : encoded;
  if (
    apiKey.length === 0 ||
    Buffer.byteLength(apiKey) > MAX_API_KEY_BYTES ||
    apiKey.trim() !== apiKey ||
    /[\u0000-\u001f\u007f]/u.test(apiKey)
  ) {
    throw new Error(
      "OpenCode Go API key must be one non-empty line without surrounding whitespace",
    );
  }
  return apiKey;
}

function unexpectedPrompt(prompt: AuthPrompt): Promise<string> {
  return Promise.reject(new Error(`OpenCode Go requested unexpected input: ${prompt.message}`));
}

main().catch((error: unknown) => {
  const failure = error instanceof Error ? error : new Error(String(error));
  console.error(failure.message);
  process.exitCode = 1;
});
