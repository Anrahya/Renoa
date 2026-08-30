import type { RegistryCandidate } from "./contract.js";

export function publisherNamespaceMatchesQuery(
  namespace: string,
  queryTokens: readonly string[],
): boolean {
  if (queryTokens.length === 0) {
    return false;
  }
  const labels = new Set(identityTokens(namespace));
  return queryTokens.every((token) => labels.has(token));
}

export function candidateMatchesQuery(
  candidate: RegistryCandidate,
  queryTokens: readonly string[],
): boolean {
  if (queryTokens.length === 0) {
    return true;
  }
  const identity = candidateIdentityTokens(candidate);
  return queryTokens.every((query) =>
    identity.some((candidateToken) => tokenMatches(query, candidateToken)),
  );
}

export function compareCandidates(
  left: RegistryCandidate,
  right: RegistryCandidate,
  queryTokens: readonly string[],
): number {
  return (
    rank(left, queryTokens) - rank(right, queryTokens) ||
    compareText(left.registry_name, right.registry_name) ||
    compareText(left.registry_version, right.registry_version)
  );
}

function rank(
  candidate: RegistryCandidate,
  queryTokens: readonly string[],
): number {
  const nameTokens = identityTokens(candidate.registry_name);
  const leafTokens = identityTokens(
    candidate.registry_name.split("/")[1] ?? candidate.registry_name,
  );
  const identity = candidateIdentityTokens(candidate);
  let score =
    candidate.status === "active"
      ? 0
      : candidate.status === "deprecated"
        ? 100
        : 200;

  if (candidate.publisher_namespace_matches_query) {
    score += 0;
  } else if (sameTokens(leafTokens, queryTokens)) {
    score += 10;
  } else if (allExact(nameTokens, queryTokens)) {
    score += 20;
  } else if (allExact(identity, queryTokens)) {
    score += 30;
  } else {
    score += 40;
  }
  if (candidate.streamable_http_count === 0) {
    score += 5;
  }
  return score;
}

function candidateIdentityTokens(
  candidate: RegistryCandidate,
): readonly string[] {
  return identityTokens(
    `${candidate.registry_name} ${candidate.title ?? ""}`,
  );
}

function identityTokens(value: string): readonly string[] {
  return value.normalize("NFKC").toLowerCase().match(/[a-z0-9]+/gu) ?? [];
}

function tokenMatches(query: string, candidate: string): boolean {
  return candidate === query || (query.length >= 4 && candidate.startsWith(query));
}

function allExact(
  identity: readonly string[],
  query: readonly string[],
): boolean {
  const tokens = new Set(identity);
  return query.length > 0 && query.every((token) => tokens.has(token));
}

function sameTokens(
  left: readonly string[],
  right: readonly string[],
): boolean {
  return left.length === right.length && left.every((token, index) => token === right[index]);
}

function compareText(left: string, right: string): number {
  return left < right ? -1 : left > right ? 1 : 0;
}
