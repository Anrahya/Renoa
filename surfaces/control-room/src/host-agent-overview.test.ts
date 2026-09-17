import { expect, it } from "vitest";
import type { HostSnapshot, Routine, Review } from "./host-contract";
import { agentOverview } from "./host-agent-overview";

it("scopes the overview to exact agent/profile relationships and orders only known timestamps", () => {
  const agent = { id: "a", name: "Same name", profile: "profile-a", created_by: null };
  const routine = (id: string, due: number, enabled = true, agent_id = "a"): Routine => ({
    id, agent_id, name: id, enabled, revision: 1, next_due_ms: due,
    schedule: { kind: "interval", hours: 1 }, pending_runs: 0, completed_runs: 0,
  });
  const review = (id: string, admitted: number, agent_id = "a"): Review => ({
    request_id: id, agent_id, admitted_at_ms: admitted, repository: "owner/repo", pull_number: 1,
    reported_head_sha: "a".repeat(40), reviewed_head_sha: null, publication: "published", worker_error: false,
    retry_after_ms: null, state: "reviewed",
  });
  const host: HostSnapshot = {
    host_id: "h", agents: [agent, { ...agent, id: "b", profile: "profile-b" }],
    routines: [routine("later", 20), routine("paused", 0, false), routine("earlier", 10), routine("unrelated", 1, true, "b")],
    reviews: [review("old", 90), review("new", 10), review("unrelated", 100, "b")],
    sessions: [{ id: "unavailable", agent_id: "a", observation: "unavailable", reason: "Storage unavailable" },
      { id: "unrelated", agent_id: "b", observation: "unavailable", reason: "Storage unavailable" }],
    connections: [{ id: "selected", catalog_available: true, tool_count: 3, selected_by_profiles: ["profile-a"] },
      { id: "other", catalog_available: true, tool_count: 5, selected_by_profiles: ["profile-b"] }],
    review_repositories: ["a", "b"].map((agent_id, index) => ({ revision: 1, policy: { repository_id: index + 1,
      installation_id: 1, full_name: agent_id, agent_id, enabled: true, triggers: ["opened"], skip_drafts: true } })),
    plugins: [], skills: [],
  };
  const original = structuredClone(host);
  const data = agentOverview(host, agent);
  expect(data.scheduled.map(r => r.id)).toEqual(["earlier", "later"]);
  expect(data.paused.map(r => r.id)).toEqual(["paused"]);
  expect(data.connections.map(c => c.id)).toEqual(["selected"]);
  expect(data.sessions.map(s => s.id)).toEqual(["unavailable"]);
  expect(data.unavailableSessions).toBe(1);
  expect(data.repositories.map(r => r.policy.agent_id)).toEqual(["a"]);
  expect(data.latestAdmission?.request_id).toBe("new");
  expect(data.latestReviews.map(r => r.request_id)).toEqual(["new"]);
  expect(host).toEqual(original);
});
