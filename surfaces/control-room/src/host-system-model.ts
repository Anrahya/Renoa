import type { Agent, HostSnapshot, Routine } from "./host-contract";

export function findAgents(agents: Agent[], query: string): Agent[] {
  const term = query.trim().toLocaleLowerCase();
  return agents.filter(agent => `${agent.name} ${agent.id}`.toLocaleLowerCase().includes(term));
}

function executionRecords(host: HostSnapshot, agent: string): string {
  return JSON.stringify([
    host.sessions.filter(s => s.agent_id === agent).map(s => s.observation === "available"
      ? [s.id, s.event_count, s.queued_operations, s.active_operation, s.latest_operation]
      : [s.id, s.observation]).sort((a, b) => String(a[0]).localeCompare(String(b[0]))),
    host.reviews.filter(r => r.agent_id === agent).map(r => [r.request_id, r.state, r.publication, r.worker_error]).sort((a, b) => String(a[0]).localeCompare(String(b[0]))),
    host.routines.filter(r => r.agent_id === agent).map(r => [r.id, r.pending_runs, r.completed_runs]).sort((a, b) => String(a[0]).localeCompare(String(b[0]))),
  ]);
}

/** A change in received records is evidence of new information, not a worker heartbeat. */
export function changedAgents(before: HostSnapshot | null, after: HostSnapshot): string[] {
  if (!before || before.host_id !== after.host_id) return [];
  return after.agents.filter(agent => before.agents.some(a => a.id === agent.id) &&
    executionRecords(before, agent.id) !== executionRecords(after, agent.id)).map(a => a.id);
}

export function scheduleCountdown(routine: Routine, now: number | null): string {
  if (routine.pending_runs > 0) return "Pending";
  if (!routine.enabled) return "Paused";
  if (now === null) return "Scheduled";
  const seconds = Math.ceil((routine.next_due_ms - now) / 1000);
  if (seconds <= 0) return "Due";
  const days = Math.floor(seconds / 86_400);
  const hours = Math.floor(seconds / 3600) % 24;
  const minutes = Math.floor(seconds / 60) % 60;
  const rest = seconds % 60;
  if (days) return `${days}d ${hours}h`;
  return `${hours ? `${hours}:` : ""}${String(minutes).padStart(2, "0")}:${String(rest).padStart(2, "0")}`;
}

export type Anchor = { x: number; y: number; width: number; height: number };
/** Both endpoints come from measured node bounds; no independent CSS half-lines. */
export function connectionPath(origin: Anchor, target: Anchor, compact: boolean): string {
  const x1 = origin.x + origin.width * (compact ? 0.5 : 1);
  const y1 = origin.y + origin.height * (compact ? 1 : 0.5);
  const x2 = target.x;
  const y2 = target.y + target.height / 2;
  return compact
    ? `M ${x1} ${y1} V ${y2 - 16} Q ${x1} ${y2} ${x1 + 16} ${y2} H ${x2}`
    : `M ${x1} ${y1} C ${(x1 + x2) / 2} ${y1}, ${(x1 + x2) / 2} ${y2}, ${x2} ${y2}`;
}
