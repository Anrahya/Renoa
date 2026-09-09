import { afterEach, expect, it, vi } from "vitest";
import { authenticatePasskey, registerPasskey, rememberedConnectionTicket } from "./passkeys";

afterEach(() => vi.unstubAllGlobals());
it("reuses a remembered owner login for a new single-use transport ticket", async () => {
  const ticket = "ab".repeat(32);
  const transport = vi.fn().mockResolvedValueOnce(Response.json({ principalId: "owner" }))
    .mockResolvedValueOnce(Response.json({ connectionTicket: ticket, expiresAtMs: Date.now() + 60_000 }));
  vi.stubGlobal("fetch", transport);
  expect((await rememberedConnectionTicket("owner"))?.connectionTicket).toBe(ticket);
  expect(transport.mock.calls[1]?.[0]).toBe("/v1/identity/connection-ticket");
});
it("distinguishes missing login from an identity service outage", async () => {
  vi.stubGlobal("fetch", vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 }))
    .mockResolvedValueOnce(new Response(null, { status: 503 })));
  expect(await rememberedConnectionTicket("owner")).toBeNull();
  await expect(rememberedConnectionTicket("owner")).rejects.toThrow("unavailable");
});

it("unwraps the Rust WebAuthn challenge envelope for both browser ceremonies", async () => {
  const options = { challenge: "public-challenge" };
  const parse = vi.fn((value: unknown) => {
    if (value !== options) throw new TypeError("WebAuthn requires the inner publicKey options");
    return value;
  });
  class BrowserCredential {
    static parseCreationOptionsFromJSON = parse;
    static parseRequestOptionsFromJSON = parse;
    toJSON() { return { id: "signed-credential" }; }
  }
  vi.stubGlobal("window", { isSecureContext: true });
  vi.stubGlobal("PublicKeyCredential", BrowserCredential);
  const credentials = { create: vi.fn().mockResolvedValue(new BrowserCredential()), get: vi.fn().mockResolvedValue(new BrowserCredential()) };
  vi.stubGlobal("navigator", { credentials });
  const grant = { connectionTicket: "ab".repeat(32), expiresAtMs: Date.now() + 60_000 };
  const response = () => ({ ok: true, json: async () => ({ ceremonyId: "ceremony", options: { publicKey: options } }) });
  vi.stubGlobal("fetch", vi.fn().mockImplementationOnce(response).mockResolvedValueOnce(Response.json(grant))
    .mockImplementationOnce(response).mockResolvedValueOnce(Response.json(grant)));
  expect(await registerPasskey("bootstrap")).toEqual(grant);
  expect(await authenticatePasskey("owner")).toEqual(grant);
  expect(credentials.create).toHaveBeenCalledWith({ publicKey: options });
  expect(credentials.get).toHaveBeenCalledWith({ publicKey: options });
});
