import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import { changedAgents, connectionPath, findAgents, openReviews, reviewStage, scheduleCountdown, scheduleSummary } from "./host-system-model";
import type { HostSnapshot, Review, Routine, Session } from "./host-contract";
import { AgentSchedules, AgentReviews } from "./host-map-branches";
import { AgentAvatar } from "./host-avatar";
import { agentActivity } from "./host-presentation";
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
  it("keeps every schedule available while surfacing pending work and the nearest due time", () => {
    const schedules = Array.from({ length: 50 }, (_, i) => ({ ...routine, id: `schedule-${i}`, name: `Job ${i}`,
      enabled: i > 5, next_due_ms: i * 1000, pending_runs: i === 4 ? 2 : 0 }));
    const summary = scheduleSummary(schedules);
    expect(summary.ordered).toHaveLength(50);
    expect(summary.ordered[0]?.id).toBe("schedule-4");
    expect(summary.next?.id).toBe("schedule-6");
    expect(summary.pending).toBe(2);
    expect(summary.paused).toBe(6);
    const html = renderToStaticMarkup(<AgentSchedules routines={schedules} now={0} />);
    expect(html).toContain('aria-expanded="false"');
    expect(html).toContain("50 schedules");
    expect(html).toContain("2 pending");
    expect(html).toContain('aria-label="Next · Job 6: 00:06"');
    expect(html).toContain('aria-label="Job 49: 00:49"');
    expect(html).toContain('inert=""');
    expect(agentActivity({ ...host, routines: [schedules[4]!] }, host.agents[0]!).tone).toBe("pending");
  });
  it("does not keep a superseded review looking active or label a prepared record as running", () => {
    const old: Review = { request_id: "old", agent_id: "sound", repository: "owner/repo", pull_number: 3,
      admitted_at_ms: 0, reported_head_sha: "sha", reviewed_head_sha: null, state: "prepared", publication: "not_recorded", worker_error: false, retry_after_ms: null };
    const finished: Review = { ...old, request_id: "new", state: "reviewed", publication: "published" };
    expect(openReviews([old, finished])).toEqual([]);
    expect(agentActivity({ ...host, reviews: [old, finished] }, host.agents[1]!).tone).toBe("quiet");
    expect(openReviews([finished, { ...old, request_id: "next" }])).toHaveLength(1);
    expect(reviewStage(old)).toBe("Prepared");
    expect(reviewStage({ ...old, worker_error: true, retry_after_ms: 100 })).toBe("Retry pending");
    const html = renderToStaticMarkup(<AgentReviews reviews={[old]} agentId="sound" />);
    expect(html).toContain("owner/repo #3");
    expect(html).toContain("Prepared");
    expect(html).not.toContain("Running");
  });
  it("keeps generic portraits stable across renames and reserves badges for recorded roles", () => {
    const render = (name: string, github = false) => renderToStaticMarkup(<AgentAvatar agentId="durable-id" name={name} github={github} />);
    expect(render("Research")).toBe(render("News Desk"));
    expect(render("Research")).toContain("/assets/identities/bots/");
    expect(render("Soundwave")).not.toContain("host-avatar-surface");
    expect(render("Research", true)).toContain("host-avatar-surface");
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
