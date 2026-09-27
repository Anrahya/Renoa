import { describe, expect, it, vi } from "vitest";
import { isBinding, parseCreationOptions, parseDiscord, pendingOperation, writeOwner } from "./agent-onboarding-client";

class MemoryStorage implements Storage {
  private values = new Map<string, string>();
  get length() { return this.values.size; }
  clear() { this.values.clear(); }
  getItem(key: string) { return this.values.get(key) ?? null; }
  key(index: number) { return [...this.values.keys()][index] ?? null; }
  removeItem(key: string) { this.values.delete(key); }
  setItem(key: string, value: string) { this.values.set(key, value); }
}
const agent = "11111111-1111-4111-8111-111111111111";
const fields = { channel_id: "123456789123456789", agent_id: agent, expected_revision: 0 };
const binding = { ...fields, channel_name: "desk", revision: 1 };
const valid = (value: unknown, request: Record<string, unknown>) => isBinding(value) && value.channel_id === request.channel_id && value.agent_id === request.agent_id;

describe("owner onboarding operations", () => {
  it("retries an uncertain save with the exact original identity and fields", async () => {
    const storage = new MemoryStorage(); let saved: Record<string, unknown> | null = null;
    const transport = vi.fn(async (_path, init) => { saved = JSON.parse(String(init?.body)); throw new Error("lost response"); }) as typeof fetch;
    expect((await writeOwner("host", "/bindings?agent=one", fields, valid, transport, storage)).kind).toBe("uncertain");
    const original = pendingOperation("host", "/bindings?agent=one", storage);
    const retry = vi.fn(async (_path, init) => {
      const request = JSON.parse(String(init?.body)); expect(request).toEqual(original);
      return new Response(JSON.stringify({ operation_id: request.operation_id, record: binding }));
    }) as typeof fetch;
    expect((await writeOwner("host", "/bindings?agent=one", { ...fields, channel_id: "999" }, valid, retry, storage)).kind).toBe("saved");
    expect(saved).toEqual(original); expect(pendingOperation("host", "/bindings?agent=one", storage)).toBeNull();
  });
  it("retains unrecognized receipts and isolates pending requests by Host and agent", async () => {
    const storage = new MemoryStorage();
    const response = vi.fn(async (_path, init) => { const request = JSON.parse(String(init?.body)); return new Response(JSON.stringify({ operation_id: request.operation_id, record: { ...binding, agent_id: "22222222-2222-4222-8222-222222222222" } })); }) as typeof fetch;
    expect((await writeOwner("host", "/bindings?agent=one", fields, valid, response, storage)).kind).toBe("uncertain");
    expect(pendingOperation("host", "/bindings?agent=one", storage)).not.toBeNull(); expect(pendingOperation("other", "/bindings?agent=one", storage)).toBeNull(); expect(pendingOperation("host", "/bindings?agent=two", storage)).toBeNull();
  });
  it("retains unauthorized work and clears only a definite rejection", async () => {
    const storage = new MemoryStorage();
    const rejected = (status: number) => vi.fn(async () => new Response(JSON.stringify({ message: "Rejected" }), { status })) as typeof fetch;
    expect((await writeOwner("host", "/bindings", fields, valid, rejected(401), storage)).kind).toBe("uncertain");
    expect(pendingOperation("host", "/bindings", storage)).not.toBeNull();
    expect((await writeOwner("host", "/bindings", fields, valid, rejected(409), storage)).kind).toBe("rejected");
    expect(pendingOperation("host", "/bindings", storage)).toBeNull();
  });
  it("does not send when this tab cannot retain the operation identity", async () => {
    const storage = new MemoryStorage(); storage.setItem = () => { throw new Error("storage full"); }; const transport = vi.fn() as typeof fetch;
    await expect(writeOwner("host", "/bindings", fields, valid, transport, storage)).rejects.toThrow("storage full"); expect(transport).not.toHaveBeenCalled();
  });
});

it("rejects incompatible discovery and binding records", () => {
  expect(parseDiscord({ status: "setup_required", bindings: [] }).status).toBe("setup_required");
  expect(() => parseDiscord({ status: "setup_required", bindings: [binding] })).toThrow();
  expect(() => parseDiscord({ status: "configured", bindings: [{ ...binding, channel_id: 123456789123456789 }] })).toThrow();
  expect(() => parseDiscord({ status: "configured", bindings: [{ ...binding, revision: 1.5 }] })).toThrow();
  expect(() => parseCreationOptions({ native_tools: ["bash"], models: [{ provider: "xai", model: "fixture", name: "Fixture", reasoning_levels: ["high"], default_reasoning: "max" }], default_model: { provider: "xai", model: "fixture", reasoning: "high" } })).toThrow();
});
