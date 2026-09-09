// Retain one in-memory request so a lost response can be retried by this browser.
// Neither the Host code nor the nonce belongs in URLs or persistent JS storage.
let pending: { pairingToken: string; browserNonce: string } | undefined;

export async function pairBrowser(pairingToken: string): Promise<void> {
  if (!window.isSecureContext) throw new Error("Open Renoa over HTTPS to pair this browser.");
  if (pending?.pairingToken !== pairingToken) {
    const bytes = crypto.getRandomValues(new Uint8Array(32));
    pending = { pairingToken, browserNonce: Array.from(bytes, byte => byte.toString(16).padStart(2, "0")).join("") };
  }
  let response: Response;
  try {
    response = await fetch("/v1/identity/pair", {
      method: "POST", credentials: "same-origin", cache: "no-store", referrerPolicy: "no-referrer", redirect: "error",
      headers: { "content-type": "application/json" }, body: JSON.stringify(pending),
    });
  } catch (cause) {
    throw new Error("Could not confirm pairing. Keep this page open and try again when the connection returns.", { cause });
  }
  if (response.status === 401) throw new Error("This pairing code has expired or was already used by another browser. Get a fresh code from your Host.");
  if (!response.ok) throw new Error("Renoa could not complete pairing. Keep this page open and try again.");
  const identity: unknown = await response.json().catch(() => undefined);
  if (typeof identity !== "object" || identity === null || !("principalId" in identity) || typeof identity.principalId !== "string") {
    throw new Error("Renoa returned an invalid pairing response. Try again without reloading this page.");
  }
  pending = undefined;
}
