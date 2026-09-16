import { describe, expect, it } from "vitest";
import type { HostSnapshot } from "./host-contract";
import { directorySummary } from "./host-agent-directory-model";
import { agentExample } from "./agent-work-preview/agent-example";

const agent = { id: "agent", name: "Agent", profile: "profile", created_by: null };
const host: HostSnapshot = { host_id: "host", agents: [agent], sessions: [], routines: [], connections: [], plugins: [], skills: [], reviews: [], review_repositories: [] };

describe("agent directory observations", () => {
  it("keeps missing live telemetry explicit instead of substituting preview activity", () => {
    const summary = directorySummary(host, agent);
    expect(summary.day).toBeUndefined();
    expect(summary.tone).toBe("quiet");
    expect(summary.title).toBe("No unfinished work recorded");
    expect(summary.next.title).toBe("No scheduled work");
  });
  it("surfaces unavailable observations even when a review was completed", () => {
    const summary = directorySummary({ ...host,
      sessions: [{ id: "session", agent_id: agent.id, observation: "unavailable", reason: "Storage unavailable" }],
      reviews: [{ request_id: "review", agent_id: agent.id, repository: "owner/repo", pull_number: 1, admitted_at_ms: 1, reported_head_sha: "abc", reviewed_head_sha: "abc", publication: "published", worker_error: false, retry_after_ms: null, state: "reviewed" }],
    }, agent);
    expect(summary.tone).toBe("interrupted");
    expect(summary.title).toBe("1 record needs attention");
    expect(summary.day).toBeUndefined();
  });
  it("keeps the interruption visible when another run completes in its hour", () => {
    const example = agentExample(agent.id);
    const summary = directorySummary(host, agent, example);
    expect(summary.day).toMatchObject({ total: 6, completed: 3, attention: 3 });
    expect(summary.day?.bins[10]).toBe("interrupted");
    expect(summary.day?.bins[11]).toBe("waiting");
    expect(summary.workHref).toBe("#agent/agent/activity/run-107");
    expect(example.executions.some(run => summary.workHref.endsWith(run.id))).toBe(true);
  });
  it("reflects paused schedules and event triggers without implying the agent is paused", () => {
    const example = agentExample(agent.id);
    const enabled = directorySummary(host, agent, example);
    expect(enabled.next.title).toContain("14:00");
    const paused = directorySummary(host, agent, { ...example, automations: example.automations.map(item => ({ ...item, enabled: false })) });
    expect(paused.next.title).toBe("Automations paused");
    expect(paused.automated).toBe(true);
    expect(paused.tone).toBe(enabled.tone);
    expect(paused.day).toEqual(enabled.day);
  });
  it("opens only records available in that agent's profile", () => {
    for (const id of ["20340f86-7f10-4c52-8757-c3124d9af0e1", "42357f5e-ae1f-0802-5218-d7f65a043086", "c8a63c3b-166d-45a0-9324-2b9db6f3d2df"]) {
      const example = agentExample(id);
      const summary = directorySummary(host, { ...agent, id }, example);
      expect(example.executions.some(run => summary.workHref.endsWith(`/${run.id}`))).toBe(true);
      for (const run of example.executions) if (run.automationId) expect(example.automations.some(item => item.id === run.automationId)).toBe(true);
    }
  });
});
