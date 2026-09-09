const SURFACE = "control_room";

interface OptionsEnvelope<T> {
  readonly ceremonyId: string;
  readonly options: { readonly publicKey: T };
}

interface TicketGrant {
  readonly connectionTicket: string;
  readonly expiresAtMs: number;
}

interface IdentityFailure {
  readonly code?: unknown;
  readonly message?: unknown;
}

// An options request consumes the bootstrap. Keep its ceremony in this page so a
// blocked/cancelled native prompt can retry without spending the bootstrap again.
// The server still enforces the ceremony's expiry and single-use verification.
let pendingRegistration: {
  readonly bootstrapToken: string;
  readonly ceremony: OptionsEnvelope<PublicKeyCredentialCreationOptionsJSON>;
} | undefined;

export async function registerPasskey(bootstrapToken: string): Promise<TicketGrant> {
  assertWebAuthnSupport();
  assertActiveDocument();
  if (pendingRegistration?.bootstrapToken !== bootstrapToken) {
    pendingRegistration = undefined;
    const ceremony = await postJson<OptionsEnvelope<PublicKeyCredentialCreationOptionsJSON>>(
      "/v1/identity/passkeys/registration/options",
      { bootstrapToken, surface: SURFACE },
    );
    pendingRegistration = { bootstrapToken, ceremony };
  }
  const { ceremony } = pendingRegistration;
  const publicKey = PublicKeyCredential.parseCreationOptionsFromJSON(ceremony.options.publicKey);
  // Fetching options may outlast a tab switch. Firefox rejects an inactive tab.
  assertActiveDocument();
  const created = await navigator.credentials.create({ publicKey }).catch(passkeyPromptFailure);
  const credential = requirePublicKeyCredential(created);
  pendingRegistration = undefined;
  const grant = await postJson<unknown>("/v1/identity/passkeys/registration/verify", {
    ceremonyId: ceremony.ceremonyId,
    credential: credential.toJSON(),
  });
  return parseTicketGrant(grant);
}

export async function authenticatePasskey(principalId: string): Promise<TicketGrant> {
  assertWebAuthnSupport();
  assertActiveDocument();
  const ceremony = await postJson<OptionsEnvelope<PublicKeyCredentialRequestOptionsJSON>>(
    "/v1/identity/passkeys/authentication/options",
    { principalId, surface: SURFACE },
  );
  const publicKey = PublicKeyCredential.parseRequestOptionsFromJSON(ceremony.options.publicKey);
  assertActiveDocument();
  const received = await navigator.credentials.get({ publicKey }).catch(passkeyPromptFailure);
  const credential = requirePublicKeyCredential(received);
  const grant = await postJson<unknown>("/v1/identity/passkeys/authentication/verify", {
    ceremonyId: ceremony.ceremonyId,
    credential: credential.toJSON(),
  });
  return parseTicketGrant(grant);
}

export async function rememberedConnectionTicket(principalId: string): Promise<TicketGrant | null> {
  const response = await fetch("/v1/identity/session", { credentials: "same-origin", cache: "no-store" });
  if (response.status === 401) return null;
  if (!response.ok) throw new Error("Renoa login service is unavailable. Reconnect when it returns.");
  const identity: unknown = await response.json();
  if (typeof identity !== "object" || identity === null || !("principalId" in identity) || identity.principalId !== principalId) return null;
  try {
    return parseTicketGrant(await postJson("/v1/identity/connection-ticket", { surface: SURFACE }));
  } catch (error) {
    if (error instanceof IdentityRequestError && error.status === 401) return null;
    throw error;
  }
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

function assertActiveDocument(): void {
  if (document.visibilityState !== "visible" || !document.hasFocus()) {
    throw new Error("Keep the Renoa tab in the foreground, then try again without reloading this page.");
  }
}

function passkeyPromptFailure(failure: unknown): never {
  if (failure instanceof DOMException && failure.name === "NotAllowedError") {
    throw new Error(
      "The passkey prompt was blocked, cancelled, or timed out. Keep Renoa in the foreground and try again without reloading this page.",
      { cause: failure },
    );
  }
  throw failure;
}

function requirePublicKeyCredential(value: Credential | null): PublicKeyCredential {
  if (!(value instanceof PublicKeyCredential)) {
    throw new Error("The passkey request was cancelled");
  }
  return value;
}

class IdentityRequestError extends Error {
  constructor(readonly status: number, message: string) { super(message); }
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
    throw new IdentityRequestError(response.status, identityError(response.status, value));
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
