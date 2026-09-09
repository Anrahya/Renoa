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
  vi.stubGlobal("document", { visibilityState: "visible", hasFocus: () => true });
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

function browser() {
  class BrowserCredential {
    static parseCreationOptionsFromJSON = (options: unknown) => options;
    static parseRequestOptionsFromJSON = (options: unknown) => options;
    toJSON() { return { id: "signed-credential" }; }
  }
  const page = { visibilityState: "visible", hasFocus: vi.fn(() => true) };
  const credentials = {
    create: vi.fn().mockResolvedValue(new BrowserCredential()),
    get: vi.fn().mockResolvedValue(new BrowserCredential()),
  };
  vi.stubGlobal("window", { isSecureContext: true });
  vi.stubGlobal("document", page);
  vi.stubGlobal("PublicKeyCredential", BrowserCredential);
  vi.stubGlobal("navigator", { credentials });
  return { page, credentials };
}

function setupTransport() {
  const grant = { connectionTicket: "ab".repeat(32), expiresAtMs: Date.now() + 60_000 };
  const options = { challenge: "public-challenge" };
  const transport = vi.fn().mockResolvedValueOnce(Response.json({
    ceremonyId: "prepared-ceremony", options: { publicKey: options },
  })).mockResolvedValueOnce(Response.json(grant));
  vi.stubGlobal("fetch", transport);
  return { transport, grant, options };
}

it.each(["hidden", "unfocused"])("does not consume a setup code from an %s page", async state => {
  const { page, credentials } = browser();
  const { transport } = setupTransport();
  if (state === "hidden") page.visibilityState = "hidden";
  else page.hasFocus.mockReturnValue(false);
  await expect(registerPasskey(`inactive-${state}`)).rejects.toThrow("foreground");
  await expect(authenticatePasskey("owner")).rejects.toThrow("foreground");
  expect(transport).not.toHaveBeenCalled();
  expect(credentials.create).not.toHaveBeenCalled();
  expect(credentials.get).not.toHaveBeenCalled();
});

it("retains prepared registration if the tab loses focus while fetching options", async () => {
  const { page, credentials } = browser();
  const { transport, grant } = setupTransport();
  page.hasFocus.mockReturnValueOnce(true).mockReturnValueOnce(false);
  await expect(registerPasskey("focus-switch")).rejects.toThrow("foreground");
  expect(transport).toHaveBeenCalledTimes(1);
  expect(credentials.create).not.toHaveBeenCalled();

  page.hasFocus.mockReturnValue(true);
  expect(await registerPasskey("focus-switch")).toEqual(grant);
  expect(transport.mock.calls.map(call => call[0])).toEqual([
    "/v1/identity/passkeys/registration/options",
    "/v1/identity/passkeys/registration/verify",
  ]);
  expect(JSON.parse(transport.mock.calls[1]?.[1].body).ceremonyId).toBe("prepared-ceremony");
});

it("retries a blocked native prompt with the same ceremony and preserves the original error", async () => {
  const { credentials } = browser();
  const { transport, grant, options } = setupTransport();
  const blocked = new DOMException("CredentialsContainer request is not allowed.", "NotAllowedError");
  credentials.create.mockRejectedValueOnce(blocked);
  await expect(registerPasskey("blocked-prompt")).rejects.toMatchObject({ cause: blocked });
  expect(transport).toHaveBeenCalledTimes(1);

  expect(await registerPasskey("blocked-prompt")).toEqual(grant);
  expect(credentials.create).toHaveBeenNthCalledWith(1, { publicKey: options });
  expect(credentials.create).toHaveBeenNthCalledWith(2, { publicKey: options });
  expect(transport).toHaveBeenCalledTimes(2);
});

it("never verifies an empty credential and permits retrying the prepared setup", async () => {
  const { credentials } = browser();
  const { transport, grant } = setupTransport();
  credentials.create.mockResolvedValueOnce(null);
  await expect(registerPasskey("empty-prompt")).rejects.toThrow("cancelled");
  expect(transport).toHaveBeenCalledTimes(1);
  expect(await registerPasskey("empty-prompt")).toEqual(grant);
});

it("uses a new ceremony when the owner supplies a replacement code", async () => {
  const { credentials } = browser();
  const { transport } = setupTransport();
  credentials.create.mockRejectedValueOnce(new DOMException("blocked", "NotAllowedError"));
  await expect(registerPasskey("old-code")).rejects.toThrow("passkey prompt");
  const replacement = { connectionTicket: "cd".repeat(32), expiresAtMs: Date.now() + 60_000 };
  transport.mockReset().mockResolvedValueOnce(Response.json({
    ceremonyId: "replacement", options: { publicKey: { challenge: "fresh" } },
  })).mockResolvedValueOnce(Response.json(replacement));
  expect(await registerPasskey("replacement-code")).toEqual(replacement);
  expect(JSON.parse(transport.mock.calls[0]?.[1].body).bootstrapToken).toBe("replacement-code");
  expect(JSON.parse(transport.mock.calls[1]?.[1].body).ceremonyId).toBe("replacement");
});
