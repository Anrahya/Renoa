import type { Agent } from "./host-contract";

export type Lineage = { roots: Agent[]; children: Map<string, Agent[]>; parents: Map<string, Agent>; detached: Set<string> };

// Creation is a recorded relationship, not ownership or a live message path.
// Missing creators and cyclic records stay discoverable without drawing a false edge.
export function agentLineage(agents: Agent[]): Lineage {
  const byId = new Map(agents.map(agent => [agent.id, agent]));
  const parents = new Map<string, Agent>();
  const detached = new Set<string>();
  for (const agent of agents) {
    if (!agent.created_by) continue;
    const seen = new Set([agent.id]);
    let current = byId.get(agent.created_by);
    let cycle = false;
    while (current) {
      if (seen.has(current.id)) { cycle = true; break; }
      seen.add(current.id);
      current = current.created_by ? byId.get(current.created_by) : undefined;
    }
    const parent = byId.get(agent.created_by);
    if (cycle || !parent) detached.add(agent.id);
    else parents.set(agent.id, parent);
  }
  const children = new Map<string, Agent[]>();
  for (const agent of agents) {
    const parent = parents.get(agent.id);
    if (parent) children.set(parent.id, [...children.get(parent.id) ?? [], agent]);
  }
  return { roots: agents.filter(agent => !parents.has(agent.id)), children, parents, detached };
}

export function agentAncestors(lineage: Lineage, id: string): Agent[] {
  const ancestors: Agent[] = [];
  let parent = lineage.parents.get(id);
  while (parent) { ancestors.unshift(parent); parent = lineage.parents.get(parent.id); }
  return ancestors;
}

export function findAgents(agents: Agent[], query: string): Agent[] {
  const term = query.trim().toLocaleLowerCase();
  return agents.filter(agent => `${agent.name} ${agent.id}`.toLocaleLowerCase().includes(term));
}
