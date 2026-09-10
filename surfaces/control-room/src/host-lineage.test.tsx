import { describe, expect, it } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { AgentMap } from "./host-agent-map";
import { agentAncestors, agentLineage, findAgents } from "./host-lineage";
import type { Agent, HostSnapshot } from "./host-contract";

const agent = (id: string, created_by: string | null = null): Agent => ({ id, created_by, name: id, profile: id });
const snapshot = (agents: Agent[]): HostSnapshot => ({ host_id: "host", agents, sessions: [], connections: [], plugins: [], skills: [], routines: [], reviews: [], review_repositories: [] });

describe("agent creation map", () => {
  it("uses recorded creators independently of agent names and roles", () => {
    const agents = [agent("operator"), agent("reviewer", "operator"), agent("researcher", "operator"), agent("news", "researcher")];
    const tree = agentLineage(agents);
    expect(tree.roots.map(a => a.id)).toEqual(["operator"]);
    expect(tree.children.get("operator")?.map(a => a.id)).toEqual(["reviewer", "researcher"]);
    expect(agentAncestors(tree, "news").map(a => a.id)).toEqual(["operator", "researcher"]);
    expect(findAgents(agents, " NEWS ")).toEqual([agents[3]]);
  });
  it("keeps missing creators and cyclic records accessible without false edges or infinite paths", () => {
    const agents = [agent("missing", "absent"), agent("self", "self"), agent("a", "b"), agent("b", "a"), agent("child", "a"), agent("valid")];
    const tree = agentLineage(agents);
    expect(tree.roots).toEqual(agents);
    expect(tree.detached.size).toBe(5);
    expect(tree.parents.size).toBe(0);
    expect(agentAncestors(tree, "child")).toEqual([]);
  });
  it("renders every direct child in a 50-agent group and preserves full identities in links", () => {
    const agents = [agent("creator"), ...Array.from({ length: 50 }, (_, i) => agent(`agent-${i}`, "creator"))];
    const html = renderToStaticMarkup(<AgentMap host={snapshot(agents)} />);
    for (const item of agents) expect(html).toContain(`href="#agent/${item.id}/work"`);
    expect(html).toContain('aria-label="Agents created by creator"');
    expect(html).toContain("Created 50 agents");
  });
  it("only assigns the GitHub role from repository policy; a Slack MCP is not a surface binding", () => {
    const host = snapshot([agent("assistant"), agent("reviewer", "assistant")]);
    host.connections = [{ id: "slack", catalog_available: true, tool_count: 27, selected_by_profiles: ["assistant"] }];
    host.review_repositories = [{ revision: 1, policy: { agent_id: "reviewer", repository_id: 1, installation_id: 1, full_name: "owner/repo", enabled: true, triggers: ["opened"], skip_drafts: false } }];
    const html = renderToStaticMarkup(<AgentMap host={host} />);
    expect(html).toContain("GitHub reviews");
    expect(html).toContain('aria-label="assistant: 1 MCP connections"');
    expect(html).not.toContain("Slack");
    expect(html).not.toContain("Live");
  });
  it("handles an empty Host", () => {
    const html = renderToStaticMarkup(<AgentMap host={snapshot([])} />);
    expect(html).toContain("No agents to display.");
  });
});
