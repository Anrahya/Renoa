import { expect, it } from "vitest";
import type { HostSnapshot, Automation } from "./host-contract";
import { agentOverview } from "./host-agent-overview";

it("scopes the overview to exact agent relationships and orders schedules by due time", () => {
  const agent = { id: "a", name: "Same name", created_by: null, preset_id: null };
  const automation = (id: string, due: number, enabled = true, agent_id = "a"): Automation => ({
    id, agent_id, name: id, enabled, revision: 1, next_due_ms: due,
    schedule: { kind: "interval", hours: 1 }, pending_runs: 0, completed_runs: 0,
  });
  const host: HostSnapshot = {
    host_id: "h", agents: [agent, { ...agent, id: "b" }],
    automations: [automation("later", 20), automation("paused", 0, false), automation("earlier", 10), automation("unrelated", 1, true, "b")],
    sessions: [{ id: "unavailable", agent_id: "a", observation: "unavailable", reason: "Storage unavailable" },
      { id: "unrelated", agent_id: "b", observation: "unavailable", reason: "Storage unavailable" }],
    connections: [{ id: "selected", catalog_available: true, tool_count: 3, selected_by_agents: ["a"] },
      { id: "other", catalog_available: true, tool_count: 5, selected_by_agents: ["b"] }],
    plugins: [], skills: [],
  };
  const original = structuredClone(host);
  const data = agentOverview(host, agent);
  expect(data.scheduled.map(r => r.id)).toEqual(["earlier", "later"]);
  expect(data.paused.map(r => r.id)).toEqual(["paused"]);
  expect(data.connections.map(c => c.id)).toEqual(["selected"]);
  expect(data.sessions.map(s => s.id)).toEqual(["unavailable"]);
  expect(data.unavailableSessions).toBe(1);
  expect(host).toEqual(original);
});
