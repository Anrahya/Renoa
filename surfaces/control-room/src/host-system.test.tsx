import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import { changedAgents, connectionPath, findAgents, scheduleCountdown, scheduleSummary } from "./host-system-model";
import type { HostSnapshot, Automation, Session } from "./host-contract";
import { AgentSchedules } from "./host-map-branches";
import { AgentAvatar } from "./host-avatar";
import { agentActivity } from "./host-presentation";
import { hostRoute } from "./host-navigation";

const automation: Automation = { id: "timer", agent_id: "rc", name: "Recap", enabled: true, revision: 1,
  schedule: { kind: "interval", hours: 12 }, next_due_ms: 61_000, pending_runs: 0, completed_runs: 0 };
const session: Session = { id: "session", agent_id: "rc", observation: "available", event_count: 2,
  queued_operations: 0, active_operation: null, latest_operation: null };
const host: HostSnapshot = { host_id: "host", agents: [
  { id: "rc", name: "Arcee", created_by: null, preset_id: "renoa.personal.arcee.v3" },
  { id: "sound", name: "Soundwave", created_by: "rc", preset_id: "renoa.general.v1" },
], sessions: [session], automations: [automation], connections: [], plugins: [], skills: [] };
afterEach(() => vi.unstubAllGlobals());

describe("System observation", () => {
  it("only signals new execution information after a baseline from the same Host", () => {
    const next = { ...host, sessions: [{ ...session, event_count: 3 }] };
    expect(changedAgents(null, next)).toEqual([]);
    expect(changedAgents(host, { ...next, host_id: "another" })).toEqual([]);
    expect(changedAgents(host, host)).toEqual([]);
    expect(changedAgents(host, next)).toEqual(["rc"]);
    expect(changedAgents(host, { ...host, automations: [{ ...automation, pending_runs: 1 }] })).toEqual(["rc"]);
    expect(changedAgents(host, { ...host, agents: host.agents.map(a => ({ ...a, name: "Renamed" })),
      automations: [{ ...automation, enabled: false, revision: 2 }] })).toEqual([]);
  });
  it("does not turn a due or paused schedule into a running worker", () => {
    expect(scheduleCountdown(automation, null)).toBe("Scheduled");
    expect(scheduleCountdown(automation, 0)).toBe("01:01");
    expect(scheduleCountdown(automation, 60_001)).toBe("00:01");
    expect(scheduleCountdown(automation, 61_000)).toBe("Due");
    expect(scheduleCountdown(automation, 9_000_000)).toBe("Due");
    expect(scheduleCountdown({ ...automation, enabled: false }, 0)).toBe("Paused");
    expect(scheduleCountdown({ ...automation, enabled: false, pending_runs: 1 }, 0)).toBe("Pending");
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
  it("keeps every schedule available while surfacing pending work and the nearest due time", () => {
    const schedules = Array.from({ length: 50 }, (_, i) => ({ ...automation, id: `schedule-${i}`, name: `Job ${i}`,
      enabled: i > 5, next_due_ms: i * 1000, pending_runs: i === 4 ? 2 : 0 }));
    const summary = scheduleSummary(schedules);
    expect(summary.ordered).toHaveLength(50);
    expect(summary.ordered[0]?.id).toBe("schedule-4");
    expect(summary.next?.id).toBe("schedule-6");
    expect(summary.pending).toBe(2);
    expect(summary.paused).toBe(6);
    const html = renderToStaticMarkup(<AgentSchedules automations={schedules} now={0} />);
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain("50 schedules");
    expect(html).toContain("2 pending");
    expect(html).toContain('aria-label="Next · Job 6: 00:06"');
    expect(html).toContain('aria-label="Job 49: 00:49"');
    expect(html).toContain('inert=""');
    expect(agentActivity({ ...host, automations: [schedules[4]!] }, host.agents[0]!).tone).toBe("pending");
  });
  it("keeps generic portraits stable across renames", () => {
    const render = (name: string) => renderToStaticMarkup(<AgentAvatar agentId="durable-id" name={name} />);
    expect(render("Research")).toBe(render("News Desk"));
    expect(render("Research")).toContain("/assets/identities/bots/");
  });
});

describe("System hierarchy and destinations", () => {
  function render(hash: string) {
    vi.stubGlobal("window", { location: { hash } });
    return renderToStaticMarkup(<HostPanelView preview host={{ status: "connected", snapshot: host,
      receivedAt: 0, error: null, refresh() {}, lock() {} }} />);
  }
  it("shows infrastructure separately from the agent map and reports health as unknown", () => {
    const html = render("#overview");
    expect(html).toContain('aria-label="System components"');
    expect(html).toContain("RCP coordinator");
    expect(html).toContain("Execution node");
    expect(html).toContain("Health unknown");
    expect(html).not.toContain('class="system-agent"');
    expect(html).not.toContain("Created by");
    expect(html).not.toContain("Core services available");
  });
  it("moves the work ledger into a separate destination", () => {
    expect(hostRoute("#work").view).toBe("work");
    const html = render("#work");
    expect(html).toMatch(/<h1[^>]*>Work<\/h1>/);
    expect(html).not.toContain('aria-label="Host system"');
    expect(html).toContain('href="#overview"');
  });
});
