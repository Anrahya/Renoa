import type { WireOAuthRegistration } from "./contract.js";
import { isLoopbackHost } from "./endpoint.js";
import { MAX_AUTH_TOKEN_BYTES, MAX_OAUTH_VALUE_BYTES } from "./limits.js";
import {
  invalid,
  requireBoundedString,
  requireExactKeys,
  requireObject,
} from "./wire-values.js";

export function parseOAuthRegistration(value: unknown): WireOAuthRegistration {
  const registration = requireObject(value, "request.registration");
  if (registration.mode === "dynamic") {
    requireExactKeys(registration, ["mode"], "request.registration");
    return { mode: "dynamic" };
  }
  if (registration.mode === "client_metadata") {
    requireExactKeys(
      registration,
      ["mode", "client_metadata_url"],
      "request.registration",
    );
    const clientMetadataUrl = requireBoundedString(
      registration.client_metadata_url,
      "request.registration.client_metadata_url",
      MAX_OAUTH_VALUE_BYTES,
    );
    let parsed: URL;
    try {
      parsed = new URL(clientMetadataUrl);
    } catch {
      throw invalid("request.registration.client_metadata_url must be a valid URL");
    }
    if (
      parsed.protocol !== "https:" ||
      parsed.pathname === "/" ||
      parsed.username.length > 0 ||
      parsed.password.length > 0 ||
      parsed.hash.length > 0
    ) {
      throw invalid(
        "request.registration.client_metadata_url must be an HTTPS URL with a non-root path and no credentials or fragment",
      );
    }
    return { mode: "client_metadata", client_metadata_url: parsed.href };
  }
  if (registration.mode === "pre_registered") {
    requireExactKeys(
      registration,
      ["mode", "issuer", "client_id", "client_secret"],
      "request.registration",
      ["client_secret"],
    );
    const issuer = parseIssuer(registration.issuer);
    const clientId = requireBoundedString(
      registration.client_id,
      "request.registration.client_id",
      MAX_OAUTH_VALUE_BYTES,
    );
    const clientSecret = registration.client_secret === undefined
      ? undefined
      : requireBoundedString(
          registration.client_secret,
          "request.registration.client_secret",
          MAX_AUTH_TOKEN_BYTES,
        );
    return {
      mode: "pre_registered",
      issuer,
      client_id: clientId,
      ...(clientSecret === undefined ? {} : { client_secret: clientSecret }),
    };
  }
  throw invalid(
    "request.registration.mode must be 'dynamic', 'client_metadata', or 'pre_registered'",
  );
}

function parseIssuer(value: unknown): string {
  const text = requireBoundedString(
    value,
    "request.registration.issuer",
    MAX_OAUTH_VALUE_BYTES,
  );
  let issuer: URL;
  try {
    issuer = new URL(text);
  } catch {
    throw invalid("request.registration.issuer must be a valid URL");
  }
  const loopback = issuer.protocol === "http:" && isLoopbackHost(issuer.hostname);
  if (
    (issuer.protocol !== "https:" && !loopback) ||
    issuer.username.length > 0 ||
    issuer.password.length > 0 ||
    issuer.search.length > 0 ||
    issuer.hash.length > 0
  ) {
    throw invalid(
      "request.registration.issuer must use HTTPS (or loopback HTTP) and contain no credentials, query, or fragment",
    );
  }
  return issuer.pathname === "/" ? issuer.origin : issuer.href;
}
