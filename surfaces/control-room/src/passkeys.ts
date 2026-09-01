const SURFACE = "control_room";

interface OptionsEnvelope<T> {
  readonly ceremonyId: string;
  readonly options: T;
}

interface TicketGrant {
  readonly connectionTicket: string;
  readonly expiresAtMs: number;
}

interface IdentityFailure {
  readonly code?: unknown;
  readonly message?: unknown;
}

export async function registerPasskey(bootstrapToken: string): Promise<TicketGrant> {
  assertWebAuthnSupport();
  const ceremony = await postJson<OptionsEnvelope<PublicKeyCredentialCreationOptionsJSON>>(
    "/v1/identity/passkeys/registration/options",
    { bootstrapToken, surface: SURFACE },
  );
  const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(ceremony.options);
  const created = await navigator.credentials.create({ publicKey });
  const credential = requirePublicKeyCredential(created);
  const grant = await postJson<unknown>("/v1/identity/passkeys/registration/verify", {
    ceremonyId: ceremony.ceremonyId,
    credential: credential.toJSON(),
  });
  return parseTicketGrant(grant);
}

export async function authenticatePasskey(principalId: string): Promise<TicketGrant> {
  assertWebAuthnSupport();
  const ceremony = await postJson<OptionsEnvelope<PublicKeyCredentialRequestOptionsJSON>>(
    "/v1/identity/passkeys/authentication/options",
    { principalId, surface: SURFACE },
  );
  const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(ceremony.options);
  const received = await navigator.credentials.get({ publicKey });
  const credential = requirePublicKeyCredential(received);
  const grant = await postJson<unknown>("/v1/identity/passkeys/authentication/verify", {
    ceremonyId: ceremony.ceremonyId,
    credential: credential.toJSON(),
  });
  return parseTicketGrant(grant);
}

export function rcpEndpoint(): string {
  const configured = import.meta.env.VITE_RENOA_RCP_ENDPOINT;
  if (typeof configured === "string" && configured !== "") {
    return configured;
  }
  const protocol = window.location.protocol === "https:" ? "wss:" : "ws:";
  return `${protocol}//${window.location.host}/connect`;
}

function assertWebAuthnSupport(): void {
  if (
    !window.isSecureContext ||
    typeof PublicKeyCredential === "undefined" ||
    typeof PublicKeyCredential.parseCreationOptionsFromJSON !== "function" ||
    typeof PublicKeyCredential.parseRequestOptionsFromJSON !== "function"
  ) {
    throw new Error("This browser cannot use Renoa passkeys from the current origin");
  }
}

function requirePublicKeyCredential(value: Credential | null): PublicKeyCredential {
  if (!(value instanceof PublicKeyCredential)) {
    throw new Error("The passkey request was cancelled");
  }
  return value;
}

async function postJson<T>(path: string, body: object): Promise<T> {
  const response = await fetch(path, {
    method: "POST",
    headers: { "content-type": "application/json" },
    body: JSON.stringify(body),
    cache: "no-store",
    credentials: "same-origin",
    referrerPolicy: "no-referrer",
  });
  const value: unknown = await response.json().catch(() => undefined);
  if (!response.ok) {
    throw new Error(identityError(response.status, value));
  }
  return value as T;
}

function identityError(status: number, value: unknown): string {
  if (typeof value === "object" && value !== null) {
    const failure = value as IdentityFailure;
    if (typeof failure.message === "string" && failure.message !== "") {
      return failure.message;
    }
  }
  return `Renoa identity request failed (${status})`;
}

function parseTicketGrant(value: unknown): TicketGrant {
  if (typeof value !== "object" || value === null) {
    throw new Error("Renoa identity returned an invalid connection ticket");
  }
  const grant = value as Readonly<Record<string, unknown>>;
  if (
    typeof grant.connectionTicket !== "string" ||
    !/^[0-9a-fA-F]{64}$/.test(grant.connectionTicket) ||
    !Number.isSafeInteger(grant.expiresAtMs)
  ) {
    throw new Error("Renoa identity returned an invalid connection ticket");
  }
  return {
    connectionTicket: grant.connectionTicket,
    expiresAtMs: grant.expiresAtMs as number,
  };
}
