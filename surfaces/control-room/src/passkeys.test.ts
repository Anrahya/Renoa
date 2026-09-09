import { afterEach, expect, it, vi } from "vitest";
import { rememberedConnectionTicket } from "./passkeys";

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
