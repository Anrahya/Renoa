import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";
import { HostFeed, type FeedState } from "./host-feed";
import { parseHost, type HostSnapshot } from "./host-contract";

const snapshot: HostSnapshot = { host_id: "00000000-0000-0000-0000-000000000001", agents: [], sessions: [],
  routines: [], connections: [], plugins: [], skills: [], reviews: [], review_repositories: [] };
const response = () => new Response(JSON.stringify(snapshot));
const flush = async () => { await vi.advanceTimersByTimeAsync(0); };

beforeEach(() => {
  vi.useFakeTimers();
  vi.stubGlobal("document", { visibilityState: "visible" });
});
afterEach(() => { vi.useRealTimers(); vi.unstubAllGlobals(); });

describe("Host continuity in a remembered browser", () => {
  it("retains received state during an outage and recovers without a login call", async () => {
    const updates: FeedState[] = [];
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(response())
      .mockRejectedValueOnce(new TypeError("Network unavailable")).mockResolvedValueOnce(response());
    const feed = new HostFeed(s => updates.push(s), fetcher);
    feed.start(); await flush();
    expect(updates.at(-1)?.status).toBe("connected");
    feed.refresh(); await flush();
    expect(updates.at(-1)?.status).toBe("reconnecting");
    expect(updates.at(-1)?.snapshot).toEqual(snapshot);
    await vi.advanceTimersByTimeAsync(2500);
    expect(updates.at(-1)?.status).toBe("connected");
    expect(fetcher.mock.calls.every(call => call[0] === "/v1/host")).toBe(true);
    feed.stop();
  });
  it("clears private state on revocation and waits for explicit authentication", async () => {
    const updates: FeedState[] = [];
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(response()).mockResolvedValueOnce(new Response(null, { status: 401 }));
    const feed = new HostFeed(s => updates.push(s), fetcher);
    feed.start(); await flush(); feed.refresh(); await flush();
    expect(updates.at(-1)).toMatchObject({ status: "locked", snapshot: null });
    await vi.advanceTimersByTimeAsync(120_000);
    expect(fetcher).toHaveBeenCalledTimes(2);
    feed.stop();
  });
  it("treats service errors and malformed snapshots as unavailable, not logged out or empty", async () => {
    const updates: FeedState[] = [];
    const fetcher = vi.fn<typeof fetch>().mockResolvedValueOnce(response())
      .mockResolvedValueOnce(new Response(null, { status: 503 }))
      .mockResolvedValueOnce(new Response(JSON.stringify({ host_id: snapshot.host_id, agents: [] })));
    const feed = new HostFeed(s => updates.push(s), fetcher);
    feed.start(); await flush(); feed.refresh(); await flush();
    expect(updates.at(-1)).toMatchObject({ status: "reconnecting", snapshot });
    feed.refresh(); await flush();
    expect(updates.at(-1)).toMatchObject({ status: "reconnecting", snapshot });
    feed.stop();
  });
  it("rejects responses from an in-flight read after explicit logout", async () => {
    const updates: FeedState[] = [];
    let resolve: ((response: Response) => void) | undefined;
    const fetcher = vi.fn<typeof fetch>(() => new Promise<Response>(done => { resolve = done; }));
    const feed = new HostFeed(s => updates.push(s), fetcher);
    feed.start(); feed.lock(); resolve?.(response()); await flush();
    expect(updates.at(-1)).toMatchObject({ status: "locked", snapshot: null });
    await vi.advanceTimersByTimeAsync(60_000);
    expect(fetcher).toHaveBeenCalledTimes(1);
    feed.stop();
  });
  it("coalesces refreshes and aborts when the view is disposed", async () => {
    let resolve: ((response: Response) => void) | undefined;
    const fetcher = vi.fn<typeof fetch>(() => new Promise<Response>(done => { resolve = done; }));
    const changed = vi.fn();
    const feed = new HostFeed(changed, fetcher);
    feed.start(); feed.refresh(); feed.refresh();
    expect(fetcher).toHaveBeenCalledTimes(1);
    feed.stop();
    expect(fetcher.mock.calls[0]?.[1]?.signal?.aborted).toBe(true);
    resolve?.(response()); await flush();
    expect(changed).not.toHaveBeenCalled();
  });
});

it("validates nested metadata before the panel consumes it", () => {
  expect(parseHost(snapshot)).toEqual(snapshot);
  expect(() => parseHost({ ...snapshot, sessions: [{ id: snapshot.host_id, agent_id: null, observation: "available" }] })).toThrow();
  expect(() => parseHost({ ...snapshot, connections: [{ id: "x-api", catalog_available: true, tool_count: 4, selected_by_profiles: null }] })).toThrow();
});
