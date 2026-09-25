import type { Agent } from "../host-contract";
import type { DirectorySummary } from "../host-agent-directory-model";
import { capabilityPlugins, pluginCapabilities } from "../agent-work-preview/configuration-model";

export type Point = { x: number; y: number };
export type SpaceAgent = {
  id: string; name: string; originalName: string; capabilityIds: string[];
  summary?: DirectorySummary; managerId?: string | undefined; synthetic?: boolean;
};
export type PlacedAgent = SpaceAgent & { position: Point; state: AgentState };
export type AgentRegion = {
  id: string; name: string; members: PlacedAgent[]; color: string; state: AgentState;
  counts: Record<AgentState, number>;
  bounds: { x: number; y: number; width: number; height: number };
};
export type AgentScene = { agents: PlacedAgent[]; regions: AgentRegion[] };

// The four states the whole control room reports, ordered by what the owner can
// do about them. An agent waiting on the owner is the healthy, actionable case
// and wears gold; only work that actually broke wears red.
export type AgentState = "needs-you" | "failed" | "running" | "idle";
export const stateRank: Record<AgentState, number> = { "needs-you": 0, failed: 1, running: 2, idle: 3 };
export const stateLabel: Record<AgentState, string> = {
  "needs-you": "Needs you", failed: "Failed", running: "Running", idle: "Idle",
};
export const emptyCounts = (): Record<AgentState, number> => ({ "needs-you": 0, failed: 0, running: 0, idle: 0 });

// A record waiting on the owner is a question, not a fault. A record that was
// interrupted is work that broke, and the two must never share a colour.
// Note the live and example summary paths currently reach "interrupted" from
// different facts, so this is the one place the two states are separated.
export function agentState(agent: SpaceAgent): AgentState {
  if (agent.synthetic) return "idle";
  switch (agent.summary?.tone) {
    case "waiting": return "needs-you";
    case "interrupted": return "failed";
    case "pending": return "running";
    default: return "idle";
  }
}

// Explicit design fixtures, not authority inferred from creation provenance.
const exampleManagement = new Map([
  ["42357f5e-ae1f-0802-5218-d7f65a043086", "20340f86-7f10-4c52-8757-c3124d9af0e1"],
  ["00000000-0000-0000-0000-000000000003", "00000000-0000-0000-0000-000000000002"],
]);
export function previewManager(agent: Agent, agents: Agent[]): string | undefined {
  const manager = exampleManagement.get(agent.id);
  return agents.some(candidate => candidate.id === manager) ? manager : undefined;
}

// Region tint reports the most urgent state among its members, so a failing or
// blocked space is legible from across the room without reading a label. An
// all-idle space stays a quiet neutral instead of a decorative colour, which is
// why the previous arbitrary seven-colour palette is gone.
const stateColors: Record<AgentState, string> = {
  "needs-you": "#d3b66f", failed: "#e08a72", running: "#8fae86", idle: "#7d7466",
};
const groupNames = ["Atlas", "Beacon", "Orbit", "Relay", "Scout", "Cedar"];

export function createScene(source: SpaceAgent[], expanded: boolean, columns = 4): AgentScene {
  const agents = source.map(agent => ({ ...agent, state: agentState(agent) }));
  if (expanded) {
    const count = Math.max(0, 50 - agents.length);
    for (let i = 0; i < count; i++) {
      const group = Math.floor(i / 8);
      agents.push({ id: `space-example-${i}`, name: i % 8 === 0 ? groupNames[group % groupNames.length]! : `${groupNames[group % groupNames.length]} ${i % 8}`,
        originalName: "Example agent", synthetic: true, state: "idle" as AgentState, managerId: i % 8 === 0 ? undefined : `space-example-${group * 8}`,
        capabilityIds: i % 3 === 0 ? ["read", "search", "mail-read"] : i % 3 === 1 ? ["read", "web", "research"] : ["web", "research", "writing"] });
    }
  }
  const roots = agents.filter(agent => !agent.managerId || !agents.some(parent => parent.id === agent.managerId));
  const regions = roots.map((root, group): AgentRegion => {
    const members = [root, ...agents.filter(agent => agent.managerId === root.id)];
    const small = !expanded && agents.length <= 6;
    const origin = small ? { x: group === 0 ? 0 : 780 + (group - 1) * 390, y: 0 } : { x: group % columns * (columns === 2 ? 950 : 760), y: Math.floor(group / columns) * 760 };
    const placed = members.map((agent, i): PlacedAgent => ({ ...agent, position: small
      ? columns === 2 ? { x: group === 0 ? 120 + i * 225 : 220, y: group === 0 ? 145 + i * 115 : 600 + (group - 1) * 380 }
        : { x: origin.x + 175 + i * 290, y: (group ? 285 : 195) + i * 165 }
      : { x: origin.x + 160 + (3 - Math.min(3, members.length)) * 105 + i % 3 * 210, y: origin.y + 185 + Math.floor(i / 3) * 200 } }));
    const counts = emptyCounts();
    for (const agent of placed) counts[agent.state] += 1;
    const state = (Object.keys(counts) as AgentState[]).reduce((worst, key) => stateRank[key] < stateRank[worst] ? key : worst, "idle" as AgentState);
    const xs = placed.map(agent => agent.position.x), ys = placed.map(agent => agent.position.y);
    return { id: root.id, name: root.name, members: placed, color: stateColors[state], state, counts,
      bounds: { x: Math.min(...xs) - 150, y: Math.min(...ys) - 150, width: Math.max(...xs) - Math.min(...xs) + 300, height: Math.max(...ys) - Math.min(...ys) + 320 } };
  });
  return { regions, agents: regions.flatMap(region => region.members) };
}

export function pluginMembers(agents: PlacedAgent[], pluginId: string): PlacedAgent[] {
  const plugin = capabilityPlugins.find(item => item.id === pluginId);
  if (!plugin) return [];
  const ids = new Set(pluginCapabilities(plugin).map(item => item.id));
  return agents.filter(agent => agent.capabilityIds.some(id => ids.has(id)));
}

// A smooth hull of the members' occupied space. Geometry describes membership;
// animating the region never moves the agents or their hit targets.
export function regionPath(points: Point[], radius = 148): string {
  if (!points.length) return "";
  const samples = points.flatMap(({ x, y }) => Array.from({ length: 12 }, (_, i) => {
    const angle = i / 12 * Math.PI * 2;
    const r = radius * (1 + .055 * Math.sin(i * 2.1));
    return { x: x + Math.cos(angle) * r, y: y + Math.sin(angle) * r * 1.09 };
  })).sort((a, b) => a.x - b.x || a.y - b.y);
  const cross = (a: Point, b: Point, c: Point) => (b.x - a.x) * (c.y - a.y) - (b.y - a.y) * (c.x - a.x);
  const half = (input: Point[]) => {
    const output: Point[] = [];
    for (const point of input) {
      while (output.length >= 2 && cross(output[output.length - 2]!, output[output.length - 1]!, point) <= 0) output.pop();
      output.push(point);
    }
    return output.slice(0, -1);
  };
  const outline = [...half(samples), ...half([...samples].reverse())];
  const center = { x: points.reduce((sum, p) => sum + p.x, 0) / points.length, y: points.reduce((sum, p) => sum + p.y, 0) / points.length };
  const hull = outline.flatMap((point, i) => {
    const next = outline[(i + 1) % outline.length]!;
    const dx = next.x - point.x, dy = next.y - point.y, length = Math.hypot(dx, dy);
    if (length < radius * 1.3) return [point];
    const side = Math.sign(-dy * (center.x - point.x) + dx * (center.y - point.y));
    const inset = Math.min(radius * .5, length * .2);
    return [point, ...[1 / 3, 2 / 3].map(t => ({ x: point.x + dx * t - dy / length * inset * side, y: point.y + dy * t + dx / length * inset * side }))];
  });
  const midpoint = (a: Point, b: Point) => `${(a.x + b.x) / 2},${(a.y + b.y) / 2}`;
  return `M${midpoint(hull[hull.length - 1]!, hull[0]!)} ${hull.map((point, i) => `Q${point.x},${point.y} ${midpoint(point, hull[(i + 1) % hull.length]!)}`).join(" ")} Z`;
}
