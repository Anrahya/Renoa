import { loadAuthStorePath } from "./config.js";
import { providerDisplayName, type ProviderId } from "./contract.js";
import { SqliteCredentialStore } from "./credentials.js";
import { ANTHROPIC_OAUTH_REFUSAL, isAnthropicOAuthToken } from "./runtime.js";
import {
  isValidVertexLocation,
  isValidVertexProject,
  VERTEX_DEFAULT_LOCATION,
  VERTEX_LOCATION_ENV,
  VERTEX_PROJECT_ENV,
} from "./providers/google-vertex.js";

/** Providers that authenticate with an API key piped on standard input. */
export type ApiKeyProvider = Extract<ProviderId, "anthropic" | "opencode-go" | "deepseek" | "openrouter" | "google-vertex">;

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
  if (provider === "anthropic" && isAnthropicOAuthToken(apiKey)) {
    throw new Error(ANTHROPIC_OAUTH_REFUSAL);
  }
  const env = provider === "google-vertex" ? vertexEnrolmentEnv(process.env) : undefined;
  const credentials = new SqliteCredentialStore(loadAuthStorePath(process.env));
  try {
    credentials.write(provider, { type: "api_key", key: apiKey, ...(env === undefined ? {} : { env }) });
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

/** The project and location a Vertex key is bound to, read at enrolment. */
function vertexEnrolmentEnv(environment: NodeJS.ProcessEnv): Record<string, string> {
  const project = environment[VERTEX_PROJECT_ENV];
  const location = environment[VERTEX_LOCATION_ENV] ?? VERTEX_DEFAULT_LOCATION;
  if (project === undefined || !isValidVertexProject(project)) {
    throw new Error(`set ${VERTEX_PROJECT_ENV} to the Google Cloud project the key belongs to`);
  }
  if (!isValidVertexLocation(location)) {
    throw new Error(`${VERTEX_LOCATION_ENV} ${location} is not a Google Cloud location`);
  }
  return { [VERTEX_PROJECT_ENV]: project, [VERTEX_LOCATION_ENV]: location };
}
