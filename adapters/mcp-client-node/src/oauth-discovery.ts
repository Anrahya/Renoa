import {
  checkResourceAllowed,
  discoverAuthorizationServerMetadata,
  discoverOAuthProtectedResourceMetadata,
  extractWWWAuthenticateParams,
  resourceUrlFromServerUrl,
  type FetchLike,
} from "@modelcontextprotocol/client";
import type { WireOAuthDiscovery } from "./contract.js";
import { AdapterProblem } from "./errors.js";
import { challengedScope } from "./oauth-scope.js";
import {
  canonicalEndpoint,
  canonicalIssuer,
  sameIssuer,
} from "./oauth-state-validation.js";

export async function discoverOAuth(
  endpoint: string,
  fetchFn: FetchLike,
): Promise<WireOAuthDiscovery> {
  const mcpEndpoint = canonicalEndpoint(endpoint);
  const challenge = await discoverOAuthChallenge(mcpEndpoint, fetchFn);
  let resource;
  try {
    resource = await discoverOAuthProtectedResourceMetadata(
      mcpEndpoint,
      challenge,
      fetchFn,
    );
  } catch (error) {
    if (
      error instanceof Error &&
      error.message ===
        "Resource server does not implement OAuth 2.0 Protected Resource Metadata."
    ) {
      throw metadataProblem(
        "The MCP endpoint does not publish OAuth protected-resource metadata.",
        "oauth_resource_metadata_missing",
      );
    }
    throw error;
  }
  if (
    !checkResourceAllowed({
      requestedResource: resourceUrlFromServerUrl(mcpEndpoint),
      configuredResource: resource.resource,
    })
  ) {
    throw metadataProblem(
      "OAuth protected-resource metadata belongs to a different MCP endpoint.",
      "oauth_resource_mismatch",
    );
  }
  const authorizationServers = resource.authorization_servers;
  if (authorizationServers === undefined) {
    throw metadataProblem(
      "The MCP endpoint does not name an OAuth authorization server.",
      "oauth_authorization_server_missing",
    );
  }
  if (authorizationServers.length !== 1) {
    throw metadataProblem(
      authorizationServers.length === 0
        ? "The MCP endpoint does not name an OAuth authorization server."
        : "The MCP endpoint names multiple OAuth authorization servers; Renoa cannot choose one safely.",
      authorizationServers.length === 0
        ? "oauth_authorization_server_missing"
        : "oauth_authorization_server_ambiguous",
    );
  }
  const selectedIssuer = canonicalIssuer(authorizationServers[0]!);
  const metadata = await discoverAuthorizationServerMetadata(selectedIssuer, {
    fetchFn,
  });
  if (metadata === undefined) {
    throw metadataProblem(
      "The authorization server does not publish valid OAuth metadata.",
      "oauth_authorization_metadata_missing",
    );
  }
  const issuer = canonicalIssuer(metadata.issuer);
  if (!sameIssuer(selectedIssuer, issuer)) {
    throw metadataProblem(
      "OAuth metadata changed the authorization server identity.",
      "oauth_issuer_mismatch",
    );
  }
  return {
    mcp_endpoint: mcpEndpoint,
    issuer,
    client_id_metadata_document_supported:
      metadata.client_id_metadata_document_supported === true,
    dynamic_client_registration_supported:
      typeof metadata.registration_endpoint === "string",
  };
}

function metadataProblem(message: string, code: string): AdapterProblem {
  return new AdapterProblem("protocol", message, { code });
}

/** Read the server's metadata hint before falling back to well-known paths. */
export async function discoverOAuthChallenge(
  endpoint: string,
  fetchFn: FetchLike,
): Promise<{ readonly resourceMetadataUrl?: URL; readonly scope?: string }> {
  const response = await fetchFn(canonicalEndpoint(endpoint), {
    method: "GET",
    headers: { accept: "application/json, text/event-stream" },
  });
  try {
    if (response.status !== 401) return {};
    const challenge = extractWWWAuthenticateParams(response);
    const scope = challengedScope(challenge.scope);
    return {
      ...(challenge.resourceMetadataUrl === undefined
        ? {}
        : { resourceMetadataUrl: challenge.resourceMetadataUrl }),
      ...(scope === undefined ? {} : { scope }),
    };
  } finally {
    await response.body?.cancel();
  }
}
