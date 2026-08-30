import type {
  RegistryCandidate,
  RegistryInputRequirement,
  RegistryLookupResult,
  RegistryPackage,
  RegistryPublisher,
  RegistryRecord,
  RegistryRemote,
  RegistryRepository,
  RegistryStatus,
  RegistryTrust,
} from "./contract.js";
import { RegistryProblem } from "./errors.js";
import {
  MAX_DESCRIPTION_BYTES,
  MAX_INPUT_ENTRIES,
  MAX_PACKAGE_ENTRIES,
  MAX_REMOTE_ENTRIES,
  MAX_TITLE_BYTES,
  MAX_URL_BYTES,
} from "./limits.js";
import { publisherNamespaceMatchesQuery } from "./ranking.js";

const REGISTRY_NAME = /^[a-zA-Z0-9.-]+\/[a-zA-Z0-9._-]+$/u;

export const REGISTRY_TRUST: RegistryTrust = {
  verified: "publisher_namespace_control",
  not_verified: [
    "provider_endorsement",
    "metadata_accuracy",
    "server_safety",
    "endpoint_behavior",
  ],
};

export interface CandidateRecord {
  readonly candidate: RegistryCandidate;
  readonly key: string;
}

export function normalizeCandidate(
  value: unknown,
  queryTokens: readonly string[],
): CandidateRecord {
  const response = object(value, "registry server response");
  const server = object(response.server, "registry server");
  const metadata = officialMetadata(response);
  if (metadata.isLatest !== true) {
    throw protocol("registry search returned a server version that is not latest");
  }
  const core = normalizeCore(server, metadata);
  const namespace = core.publisher.namespace.toLowerCase();
  const remotes = optionalArray(server.remotes, "registry server remotes");
  const packages = optionalArray(server.packages, "registry server packages");
  boundedEntries(remotes, MAX_REMOTE_ENTRIES, "registry server remotes");
  boundedEntries(packages, MAX_PACKAGE_ENTRIES, "registry server packages");
  const candidate: RegistryCandidate = {
    registry_name: core.name,
    registry_version: core.version,
    ...(core.title === undefined ? {} : { title: core.title }),
    publisher_description: core.description,
    publisher: core.publisher,
    publisher_namespace_matches_query: publisherNamespaceMatchesQuery(
      namespace,
      queryTokens,
    ),
    status: core.status,
    ...(core.websiteUrl === undefined
      ? {}
      : { website_url: core.websiteUrl }),
    ...(core.repository === undefined
      ? {}
      : { repository: core.repository }),
    remote_count: remotes.length,
    streamable_http_count: remotes.filter(
      (remote) =>
        typeof remote === "object" &&
        remote !== null &&
        !Array.isArray(remote) &&
        (remote as Record<string, unknown>).type === "streamable-http",
    ).length,
    package_count: packages.length,
  };
  return { candidate, key: `${core.name}\u0000${core.version}` };
}

export function normalizeLookup(
  value: unknown,
  expectedName: string,
  expectedVersion: string,
  sourceRecord: string,
): RegistryLookupResult {
  const response = object(value, "registry server response");
  const server = object(response.server, "registry server");
  const metadata = officialMetadata(response);
  const core = normalizeCore(server, metadata);
  if (core.name !== expectedName || core.version !== expectedVersion) {
    throw protocol(
      "registry lookup returned a different server name or version than requested",
    );
  }
  const remotes = optionalArray(server.remotes, "registry server remotes");
  const packages = optionalArray(server.packages, "registry server packages");
  boundedEntries(remotes, MAX_REMOTE_ENTRIES, "registry server remotes");
  boundedEntries(packages, MAX_PACKAGE_ENTRIES, "registry server packages");
  const record: RegistryRecord = {
    registry_name: core.name,
    registry_version: core.version,
    ...(core.title === undefined ? {} : { title: core.title }),
    publisher_description: core.description,
    publisher: core.publisher,
    status: core.status,
    ...(core.websiteUrl === undefined
      ? {}
      : { website_url: core.websiteUrl }),
    ...(core.repository === undefined
      ? {}
      : { repository: core.repository }),
    remotes: remotes.map((remote, index) =>
      normalizeRemote(remote, `registry remote ${index}`),
    ),
    packages: packages.map((item, index) =>
      normalizePackage(item, `registry package ${index}`),
    ),
    source_record: sourceRecord,
  };
  return {
    action: "lookup",
    source: "official_mcp_registry",
    record,
    trust: REGISTRY_TRUST,
    next_action:
      "Treat this as publisher metadata only. Verify the selected endpoint and authentication against the provider's official HTTPS documentation. Then call add with kind=mcp and the exact reviewed values; never copy secret header values from registry metadata.",
  };
}

interface CoreRecord {
  readonly name: string;
  readonly version: string;
  readonly title?: string;
  readonly description: string;
  readonly publisher: RegistryPublisher;
  readonly status: RegistryStatus;
  readonly websiteUrl?: string;
  readonly repository?: RegistryRepository;
}

function normalizeCore(
  server: Record<string, unknown>,
  metadata: Record<string, unknown>,
): CoreRecord {
  const name = boundedString(server.name, "registry server name", 3, 200);
  if (!REGISTRY_NAME.test(name)) {
    throw protocol("registry server name is malformed");
  }
  const namespace = name.slice(0, name.indexOf("/"));
  const version = boundedString(server.version, "registry server version", 1, 255);
  if (version === "latest" || /[\u0000-\u001F\u007F/]/u.test(version)) {
    throw protocol("registry server version is not an exact safe version");
  }
  const description = cleanPublisherText(
    server.description,
    "registry server description",
    MAX_DESCRIPTION_BYTES,
  );
  const title = optionalPublisherText(
    server.title,
    "registry server title",
    MAX_TITLE_BYTES,
  );
  const websiteUrl = optionalHttpsUrl(server.websiteUrl, "registry website URL");
  const repository = normalizeRepository(server.repository);
  return {
    name,
    version,
    ...(title === undefined ? {} : { title }),
    description,
    publisher: {
      namespace,
      verification: namespace.startsWith("io.github.") ? "github" : "domain",
    },
    status: status(metadata.status),
    ...(websiteUrl === undefined ? {} : { websiteUrl }),
    ...(repository === undefined ? {} : { repository }),
  };
}

function normalizeRepository(value: unknown): RegistryRepository | undefined {
  if (value === undefined) {
    return undefined;
  }
  const repository = object(value, "registry repository");
  const url = optionalHttpsUrl(repository.url, "registry repository URL");
  if (url === undefined) {
    return undefined;
  }
  const source = boundedString(repository.source, "registry repository source", 1, 64);
  const id = optionalBoundedString(repository.id, "registry repository id", 256);
  return { url, source, ...(id === undefined ? {} : { id }) };
}

function normalizeRemote(value: unknown, path: string): RegistryRemote {
  const remote = object(value, path);
  const declaredTransport = boundedString(remote.type, `${path} transport`, 1, 64);
  const transport =
    declaredTransport === "streamable-http" || declaredTransport === "sse"
      ? declaredTransport
      : "unknown";
  const url = boundedString(remote.url, `${path} URL`, 1, MAX_URL_BYTES);
  if (/\s|[\u0000-\u001F\u007F]/u.test(url)) {
    throw protocol(`${path} URL contains whitespace or control characters`);
  }
  const templateSafeUrl = url.replace(/\{[a-zA-Z0-9._-]+\}/gu, "template-value");
  const hasTemplate = templateSafeUrl !== url;
  if (/[{}]/u.test(templateSafeUrl)) {
    throw protocol(`${path} URL contains a malformed template variable`);
  }
  let parsed: URL;
  try {
    parsed = new URL(templateSafeUrl);
  } catch (error) {
    throw new RegistryProblem("protocol", `${path} URL is invalid`, {
      code: "invalid_registry_record",
      cause: error,
    });
  }
  if (
    parsed.username.length > 0 ||
    parsed.password.length > 0 ||
    parsed.hash.length > 0
  ) {
    throw protocol(`${path} URL contains credentials or a fragment`);
  }
  if (!hasTemplate && parsed.search.length > 0) {
    throw protocol(`${path} URL contains concrete query parameters`);
  }
  const isHttps = parsed.protocol === "https:";
  const blocker =
    transport === "unknown"
      ? "unsupported_transport"
      : transport === "sse"
      ? "sse_transport_unsupported"
      : hasTemplate
        ? "endpoint_template_unsupported"
        : !isHttps
          ? "https_required"
          : undefined;
  const headers = optionalArray(remote.headers, `${path} headers`);
  boundedEntries(headers, MAX_INPUT_ENTRIES, `${path} headers`);
  return {
    transport,
    ...(transport === "unknown"
      ? { declared_transport: declaredTransport }
      : {}),
    url,
    transport_supported: blocker === undefined,
    ...(blocker === undefined ? {} : { blocker }),
    headers: headers.map((header, index) =>
      normalizeInput(header, `${path} header ${index}`),
    ),
  };
}

function normalizeInput(value: unknown, path: string): RegistryInputRequirement {
  const input = object(value, path);
  const name = boundedString(input.name, `${path} name`, 1, 256);
  const description = optionalPublisherText(
    input.description,
    `${path} description`,
    512,
  );
  return {
    name,
    required: optionalBoolean(input.isRequired, `${path} isRequired`) ?? false,
    secret: optionalBoolean(input.isSecret, `${path} isSecret`) ?? false,
    ...(description === undefined ? {} : { description }),
  };
}

function cleanPublisherText(value: unknown, path: string, maximum: number): string {
  const text = boundedString(value, path, 1, maximum)
    .replace(/[\u0000-\u001F\u007F]+/gu, " ")
    .replace(/\s+/gu, " ")
    .trim();
  if (text.length === 0) {
    throw protocol(`${path} is empty after safe text normalization`);
  }
  return text;
}

function optionalPublisherText(
  value: unknown,
  path: string,
  maximum: number,
): string | undefined {
  return value === undefined ? undefined : cleanPublisherText(value, path, maximum);
}

function normalizePackage(value: unknown, path: string): RegistryPackage {
  const item = object(value, path);
  const registryType = boundedString(item.registryType, `${path} registryType`, 1, 64);
  const identifier = boundedString(item.identifier, `${path} identifier`, 1, 2_048);
  validatePackageIdentifier(identifier, path);
  const version = optionalBoundedString(item.version, `${path} version`, 255);
  const transportValue = object(item.transport, `${path} transport`).type;
  const transport =
    transportValue === "stdio" ||
    transportValue === "streamable-http" ||
    transportValue === "sse"
      ? transportValue
      : "unknown";
  return {
    registry_type: registryType,
    identifier,
    ...(version === undefined ? {} : { version }),
    transport,
    supported_by_renoa: false,
    blocker: "local_package_execution_not_supported",
  };
}

function validatePackageIdentifier(identifier: string, path: string): void {
  if (/[\u0000-\u001F\u007F]/u.test(identifier)) {
    throw protocol(`${path} identifier contains control characters`);
  }
  if (!/^https?:\/\//iu.test(identifier)) {
    return;
  }
  let url: URL;
  try {
    url = new URL(identifier);
  } catch (error) {
    throw new RegistryProblem("protocol", `${path} identifier URL is invalid`, {
      code: "invalid_registry_record",
      cause: error,
    });
  }
  if (
    url.protocol !== "https:" ||
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.search.length > 0 ||
    url.hash.length > 0
  ) {
    throw protocol(`${path} identifier URL is not safe to expose`);
  }
}

function officialMetadata(response: Record<string, unknown>): Record<string, unknown> {
  const meta = object(response._meta, "registry response metadata");
  return object(
    meta["io.modelcontextprotocol.registry/official"],
    "official registry metadata",
  );
}

function status(value: unknown): RegistryStatus {
  if (value === "active" || value === "deprecated" || value === "deleted") {
    return value;
  }
  throw protocol("official registry status is not active, deprecated, or deleted");
}

function optionalHttpsUrl(value: unknown, path: string): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  const text = boundedString(value, path, 1, MAX_URL_BYTES);
  let url: URL;
  try {
    url = new URL(text);
  } catch {
    return undefined;
  }
  if (
    url.protocol !== "https:" ||
    url.username.length > 0 ||
    url.password.length > 0 ||
    url.hash.length > 0 ||
    url.search.length > 0
  ) {
    return undefined;
  }
  return url.toString();
}

function object(value: unknown, path: string): Record<string, unknown> {
  if (typeof value !== "object" || value === null || Array.isArray(value)) {
    throw protocol(`${path} must be an object`);
  }
  return value as Record<string, unknown>;
}

function optionalArray(value: unknown, path: string): readonly unknown[] {
  if (value === undefined) {
    return [];
  }
  if (!Array.isArray(value)) {
    throw protocol(`${path} must be an array`);
  }
  return value;
}

function boundedEntries(value: readonly unknown[], maximum: number, path: string): void {
  if (value.length > maximum) {
    throw new RegistryProblem(
      "resource_limit",
      `${path} contains more than ${maximum} entries.`,
      { code: "registry_record_limit" },
    );
  }
}

function boundedString(
  value: unknown,
  path: string,
  minimum: number,
  maximum: number,
): string {
  if (typeof value !== "string") {
    throw protocol(`${path} must be a string`);
  }
  const length = Buffer.byteLength(value, "utf8");
  if (length < minimum || length > maximum) {
    throw protocol(`${path} must contain ${minimum}-${maximum} UTF-8 bytes`);
  }
  return value;
}

function optionalBoundedString(
  value: unknown,
  path: string,
  maximum: number,
): string | undefined {
  if (value === undefined) {
    return undefined;
  }
  return boundedString(value, path, 1, maximum);
}

function optionalBoolean(value: unknown, path: string): boolean | undefined {
  if (value === undefined) {
    return undefined;
  }
  if (typeof value !== "boolean") {
    throw protocol(`${path} must be a boolean`);
  }
  return value;
}

function protocol(message: string): RegistryProblem {
  return new RegistryProblem("protocol", message, {
    code: "invalid_registry_record",
  });
}
