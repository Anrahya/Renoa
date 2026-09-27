import type { DirectorySummary } from "../host-agent-directory-model";
import { emptyCounts, stateRank, toneState, type AgentState } from "../host-state";

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
/** One shared time axis for the scene, so every agent's strip is comparable. */
export type DayAxis = { start: number; end: number } | null;
export type AgentScene = { agents: PlacedAgent[]; regions: AgentRegion[]; axis: DayAxis };

export const agentState = (agent: SpaceAgent): AgentState => toneState(agent.summary?.tone, agent.synthetic);

/** The span covering every recorded mark, so two agents' strips line up. */
export function dayAxis(agents: PlacedAgent[]): DayAxis {
  const times = agents.flatMap(agent => agent.summary?.day ?? []).map(mark => mark.at);
  if (!times.length) return null;
  const start = Math.min(...times) - 60_000;
  const end = Math.max(...times) + 60_000;
  return { start, end: Math.max(start + 1, end) };
}


// Region tint reports the most urgent state among its members, so a failing or
// blocked space is legible from across the room without reading a label. An
// all-idle space stays a quiet neutral instead of a decorative colour, which is
// why the previous arbitrary seven-colour palette is gone.
const stateColors: Record<AgentState, string> = {
  "needs-you": "#d3b66f", failed: "#e08a72", running: "#8fae86", idle: "#7d7466",
};
export function createScene(source: SpaceAgent[], columns = 4): AgentScene {
  const agents = source.map(agent => ({ ...agent, state: agentState(agent) }));
  const roots = agents.filter(agent => !agent.managerId || !agents.some(parent => parent.id === agent.managerId));
  const regions = roots.map((root, group): AgentRegion => {
    const members = [root, ...agents.filter(agent => agent.managerId === root.id)];
    const small = agents.length <= 6;
    const origin = small ? { x: group === 0 ? 0 : 780 + (group - 1) * 390, y: 0 } : { x: group % columns * (columns === 2 ? 950 : 760), y: Math.floor(group / columns) * 760 };
    const placed = members.map((agent, i): PlacedAgent => ({ ...agent, position: small
      ? columns === 2 ? { x: group === 0 ? 120 + i * 225 : 220, y: group === 0 ? 145 + i * 115 : 600 + (group - 1) * 380 }
        : { x: origin.x + 175 + i * 290, y: (group ? 285 : 195) + i * 165 }
      : { x: origin.x + 160 + (3 - Math.min(3, members.length)) * 105 + i % 3 * 210, y: origin.y + 185 + Math.floor(i / 3) * 200 } }));
    const counts = emptyCounts();
    for (const agent of placed) counts[agent.state] += 1;
    const state = (Object.keys(counts) as AgentState[]).reduce((worst, key) => counts[key] > 0 && stateRank[key] < stateRank[worst] ? key : worst, "idle" as AgentState);
    const xs = placed.map(agent => agent.position.x), ys = placed.map(agent => agent.position.y);
    return { id: root.id, name: root.name, members: placed, color: stateColors[state], state, counts,
      bounds: { x: Math.min(...xs) - 150, y: Math.min(...ys) - 150, width: Math.max(...xs) - Math.min(...xs) + 300, height: Math.max(...ys) - Math.min(...ys) + 320 } };
  });
  const placed = regions.flatMap(region => region.members);
  return { regions, agents: placed, axis: dayAxis(placed) };
}

export type SpacePlugin = { id: string; name: string; capabilityIds: string[] };
export function pluginMembers(agents: PlacedAgent[], plugin: SpacePlugin | undefined): PlacedAgent[] {
  if (!plugin) return [];
  const ids = new Set(plugin.capabilityIds);
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
