import { loadAuthStorePath } from "./config.js";
import { providerDisplayName, type ProviderId } from "./contract.js";
import { SqliteCredentialStore } from "./credentials.js";

/** Providers that authenticate with an API key piped on standard input. */
export type ApiKeyProvider = Extract<ProviderId, "opencode-go" | "deepseek">;

/** Stores the API key read from standard input; the key is never printed. */
export function runApiKeyAuth(provider: ApiKeyProvider): void {
  enroll(provider).catch((error: unknown) => {
    const failure = error instanceof Error ? error : new Error(String(error));
    process.stderr.write(`${failure.message}\n`);
    process.exitCode = 1;
  });
}

const MAX_API_KEY_BYTES = 4_096;

async function enroll(provider: ApiKeyProvider): Promise<void> {
  const label = providerDisplayName(provider);
  if (process.stdin.isTTY) {
    throw new Error(
      `${label} authentication requires an API key piped on standard input; see adapters/model-provider-node/README.md`,
    );
  }
  process.stdin.setEncoding("utf8");
  const apiKey = await readApiKey(process.stdin, label);
  const credentials = new SqliteCredentialStore(loadAuthStorePath(process.env));
  try {
    credentials.write(provider, { type: "api_key", key: apiKey });
    process.stdout.write(`${label} API key stored.\n`);
  } finally {
    credentials.close();
  }
}

async function readApiKey(input: AsyncIterable<unknown>, label: string): Promise<string> {
  let encoded = "";
  let bytes = 0;
  for await (const chunk of input) {
    if (typeof chunk !== "string") {
      throw new Error(`${label} API key input is not UTF-8 text`);
    }
    bytes += Buffer.byteLength(chunk);
    if (bytes > MAX_API_KEY_BYTES + 2) {
      throw new Error(`${label} API key exceeds ${MAX_API_KEY_BYTES} bytes`);
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
    throw new Error(`${label} API key must be one non-empty line without surrounding whitespace`);
  }
  return apiKey;
}
