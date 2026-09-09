import { describe, expect, it, vi } from "vitest";
import { pendingChange, saveChange } from "./host-mutations";

function memory(): Storage {
  const values = new Map<string, string>();
  return { getItem: key => values.get(key) ?? null, setItem: (key, value) => { values.set(key, value); },
    removeItem: key => { values.delete(key); }, clear: () => values.clear(), key: index => [...values.keys()][index] ?? null,
    get length() { return values.size; } };
}
const path = "/v1/host/routines/routine/enabled";
describe("owner changes", () => {
  it("recovers a lost acknowledgment after reload with the same operation and original input", async () => {
    const storage = memory();
    const sent: string[] = [];
    const lost = vi.fn(async (_: unknown, init?: RequestInit) => { sent.push(String(init?.body)); throw new TypeError("disconnected"); });
    expect((await saveChange("host", path, { expected_revision: 2, enabled: false }, lost, storage)).kind).toBe("uncertain");
    expect(pendingChange("host", path, storage)?.enabled).toBe(false);
    const retry = vi.fn(async (_: unknown, init?: RequestInit) => {
      sent.push(String(init?.body)); const request = JSON.parse(String(init?.body));
      return Response.json({ operation_id: request.operation_id, id: "routine", revision: 3, enabled: false, next_due_ms: 0 });
    });
    expect((await saveChange("host", path, { expected_revision: 20, enabled: true }, retry, storage)).kind).toBe("saved");
    expect(sent[0]).toBe(sent[1]);
    expect(pendingChange("host", path, storage)).toBeNull();
  });
  it("retains uncertain or mismatched receipts but discards definitive revision rejection", async () => {
    const storage = memory();
    const unrecognized = vi.fn().mockResolvedValue(Response.json({ operation_id: "another-operation" }));
    expect((await saveChange("host", path, { expected_revision: 1, enabled: false }, unrecognized, storage)).kind).toBe("uncertain");
    expect(pendingChange("host", path, storage)).not.toBeNull();
    expect(pendingChange("other-host", path, storage)).toBeNull();
    expect((await saveChange("host", path, {}, vi.fn().mockResolvedValue(new Response(null, { status: 409 })), storage)).kind).toBe("rejected");
    expect(pendingChange("host", path, storage)).toBeNull();
  });
  it("does not send if storage cannot preserve the operation identity", async () => {
    const storage = memory(); storage.setItem = () => { throw new Error("disk full"); };
    const send = vi.fn();
    await expect(saveChange("host", path, { expected_revision: 1 }, send, storage)).rejects.toThrow("disk full");
    expect(send).not.toHaveBeenCalled();
  });
});
