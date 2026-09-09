import { afterEach, expect, it, vi } from "vitest";
import { pairBrowser } from "./browser-pairing";

afterEach(() => vi.unstubAllGlobals());

it("retries a lost pairing response with the same browser proof", async () => {
  vi.stubGlobal("window", { isSecureContext: true });
  const transport = vi.fn().mockRejectedValueOnce(new TypeError("connection lost"))
    .mockResolvedValueOnce(Response.json({ principalId: "owner" }));
  vi.stubGlobal("fetch", transport);
  await expect(pairBrowser("lost-response")).rejects.toThrow("try again");
  await pairBrowser("lost-response");
  const first = transport.mock.calls[0]?.[1];
  expect(first.credentials).toBe("same-origin");
  expect(JSON.parse(first.body).browserNonce).toMatch(/^[0-9a-f]{64}$/);
  expect(transport.mock.calls[1]?.[1].body).toBe(first.body);
});

it("does not interpret rejection or an outage as successful pairing", async () => {
  vi.stubGlobal("window", { isSecureContext: true });
  const transport = vi.fn().mockResolvedValueOnce(new Response(null, { status: 401 }))
    .mockResolvedValueOnce(new Response(null, { status: 503 }))
    .mockResolvedValueOnce(Response.json({ principalId: "owner" }));
  vi.stubGlobal("fetch", transport);
  await expect(pairBrowser("expired-code")).rejects.toThrow("fresh code");
  await expect(pairBrowser("replacement-code")).rejects.toThrow("try again");
  await pairBrowser("replacement-code");
  expect(transport.mock.calls[0]?.[1].body).not.toBe(transport.mock.calls[1]?.[1].body);
  expect(transport.mock.calls[1]?.[1].body).toBe(transport.mock.calls[2]?.[1].body);
});

it("does not send the Host code from an insecure origin", async () => {
  vi.stubGlobal("window", { isSecureContext: false });
  const transport = vi.fn();
  vi.stubGlobal("fetch", transport);
  await expect(pairBrowser("secret-code")).rejects.toThrow("HTTPS");
  expect(transport).not.toHaveBeenCalled();
});
