import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import { changedAgents, connectionPath, findAgents, scheduleCountdown } from "./host-system-model";
import type { HostSnapshot, Routine, Session } from "./host-contract";
import { hostRoute } from "./host-navigation";

const routine: Routine = { id: "timer", agent_id: "rc", name: "Recap", enabled: true, revision: 1,
  schedule: { kind: "interval", hours: 12 }, next_due_ms: 61_000, pending_runs: 0, completed_runs: 0 };
const session: Session = { id: "session", agent_id: "rc", observation: "available", event_count: 2,
  queued_operations: 0, active_operation: null, latest_operation: null };
const host: HostSnapshot = { host_id: "host", agents: [
  { id: "rc", name: "Arcee", profile: "operator", created_by: null },
  { id: "sound", name: "Soundwave", profile: "review", created_by: "rc" },
], sessions: [session], routines: [routine], reviews: [], review_repositories: [], connections: [], plugins: [], skills: [] };
afterEach(() => vi.unstubAllGlobals());

describe("System observation", () => {
  it("only signals new execution information after a baseline from the same Host", () => {
    const next = { ...host, sessions: [{ ...session, event_count: 3 }] };
    expect(changedAgents(null, next)).toEqual([]);
    expect(changedAgents(host, { ...next, host_id: "another" })).toEqual([]);
    expect(changedAgents(host, host)).toEqual([]);
    expect(changedAgents(host, next)).toEqual(["rc"]);
    expect(changedAgents(host, { ...host, routines: [{ ...routine, pending_runs: 1 }] })).toEqual(["rc"]);
    expect(changedAgents(host, { ...host, agents: host.agents.map(a => ({ ...a, name: "Renamed" })),
      routines: [{ ...routine, enabled: false, revision: 2 }] })).toEqual([]);
  });
  it("does not turn a due or paused schedule into a running worker", () => {
    expect(scheduleCountdown(routine, null)).toBe("Scheduled");
    expect(scheduleCountdown(routine, 0)).toBe("01:01");
    expect(scheduleCountdown(routine, 60_001)).toBe("00:01");
    expect(scheduleCountdown(routine, 61_000)).toBe("Due");
    expect(scheduleCountdown(routine, 9_000_000)).toBe("Due");
    expect(scheduleCountdown({ ...routine, enabled: false }, 0)).toBe("Paused");
    expect(scheduleCountdown({ ...routine, enabled: false, pending_runs: 1 }, 0)).toBe("Pending");
  });
  it("preserves exact node endpoints in desktop and stacked layouts", () => {
    const origin = { x: 20, y: 30, width: 112, height: 112 };
    const target = { x: 400, y: 60, width: 64, height: 64 };
    expect(connectionPath(origin, target, false)).toBe("M 132 86 C 266 86, 266 92, 400 92");
    expect(connectionPath({ x: 0, y: 0, width: 72, height: 72 },
      { x: 64, y: 110, width: 48, height: 48 }, true)).toBe("M 36 72 V 118 Q 36 134 52 134 H 64");
  });
  it("keeps all agents searchable, including larger inventories", () => {
    const agents = Array.from({ length: 50 }, (_, i) => ({ ...host.agents[0]!, id: `id-${i}`, name: `Research ${i}` }));
    expect(findAgents(agents, "")).toHaveLength(50);
    expect(findAgents(agents, " RESEARCH 49 ").map(a => a.id)).toEqual(["id-49"]);
  });
});

describe("System hierarchy and destinations", () => {
  function render(hash: string) {
    vi.stubGlobal("window", { location: { hash } });
    return renderToStaticMarkup(<HostPanelView preview host={{ status: "connected", snapshot: host,
      receivedAt: 0, error: null, refresh() {}, lock() {} }} />);
  }
  it("shows created agents as Host peers and schedules under their target", () => {
    const html = render("#overview");
    const rows = html.split('<li class="system-agent">').slice(1);
    expect(rows).toHaveLength(2);
    expect(rows[0]).toContain('data-agent-anchor="rc"');
    expect(rows[0]).toContain('aria-label="Recap: 01:01"');
    expect(rows[1]).toContain('data-agent-anchor="sound"');
    expect(rows[1]).not.toContain("Recap");
    expect(html).not.toContain("Shared library");
    expect(html).not.toContain("Review history");
    expect(html).not.toContain("Created by");
  });
  it("moves the work ledger into a separate destination", () => {
    expect(hostRoute("#work").view).toBe("work");
    const html = render("#work");
    expect(html).toContain("<h1>Work</h1>");
    expect(html).not.toContain('aria-label="Host system"');
    expect(html).toContain('href="#overview"');
  });
});
