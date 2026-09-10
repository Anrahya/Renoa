import type { Agent, Connection, HostSnapshot, Review, Routine, Session } from "./host-contract";

export const isEarlier = (agent: Agent) => agent.name.startsWith("renoa.");
export const displayName = (name: string) => name.startsWith("renoa.") ? "Earlier agent record" : name;
export function agentName(host: HostSnapshot, id: string | null): string {
  const agent = host.agents.find(a => a.id === id);
  return agent ? displayName(agent.name) : "Unassigned session";
}
export function timestamp(value: number): string {
  return new Intl.DateTimeFormat(undefined, { dateStyle: "medium", timeStyle: "short" }).format(value);
}
export function scheduleText(routine: Routine): string {
  const s = routine.schedule;
  if (s.kind === "interval") return `Every ${s.hours} ${s.hours === 1 ? "hour" : "hours"}`;
  if (s.kind === "once") return `Once · ${timestamp(Date.parse(s.at))}`;
  return `Daily · ${String(s.hour).padStart(2, "0")}:${String(s.minute).padStart(2, "0")} · ${s.timezone}`;
}
export function needsAttention(review: Review): boolean {
  return review.state === "incomplete" || review.publication === "needs_attention" ||
    review.worker_error && ["not_recorded", "sending"].includes(review.publication);
}
export function currentReviews(reviews: Review[]): Review[] {
  // Catalog order is admission order, not completion time. Newer admissions
  // move older attempts to history without claiming their failures resolved.
  const seen = new Set<string>();
  return [...reviews].reverse().filter(review => {
    const key = `${review.repository}:${review.pull_number}`;
    if (seen.has(key)) return false;
    seen.add(key); return true;
  });
}
export function attentionReviews(reviews: Review[]): Review[] {
  const latest = new Set(currentReviews(reviews));
  // Publication ambiguity can need intervention on an earlier attempt, too.
  return [...reviews].reverse().filter(r => latest.has(r) && needsAttention(r) ||
    r.publication === "needs_attention" || r.worker_error && ["not_recorded", "sending"].includes(r.publication));
}
export function sessionNeedsAttention(session: Session): boolean {
  if (session.observation === "unavailable") return true;
  const operation = session.active_operation ?? session.latest_operation;
  return operation?.state === "failed" || operation?.state === "outcome_unknown";
}
export function sessionUnfinished(session: Session): boolean {
  return session.observation === "available" && (!!session.active_operation || session.queued_operations > 0);
}
export function agentActivity(host: HostSnapshot, agent: Agent) {
  const reviews = host.reviews.filter(r => r.agent_id === agent.id);
  const sessions = host.sessions.filter(s => s.agent_id === agent.id);
  const attention = attentionReviews(reviews).length + sessions.filter(sessionNeedsAttention).length;
  const unfinished = currentReviews(host.reviews).filter(r => r.agent_id === agent.id && ["queued", "prepared"].includes(r.state)).length +
    sessions.filter(sessionUnfinished).length + host.routines.filter(r => r.agent_id === agent.id).reduce((count, r) => count + r.pending_runs, 0);
  const next = host.routines.filter(r => r.agent_id === agent.id && r.enabled).sort((a, b) => a.next_due_ms - b.next_due_ms)[0];
  if (attention) return { tone: "attention", label: `${attention} ${attention === 1 ? "record needs" : "records need"} attention` };
  if (unfinished) return { tone: "pending", label: `${unfinished} unfinished ${unfinished === 1 ? "record" : "records"}` };
  if (next) return { tone: "scheduled", label: `Next · ${timestamp(next.next_due_ms)}` };
  return { tone: "quiet", label: "No unfinished work recorded" };
}
export function connectionName(host: HostSnapshot, connection: Connection): string {
  // IDs from plugins/manager/identity.rs carry 24 digest characters. Only
  // label a unique match; the full connection identity remains in its detail.
  const match = /^plugin\.([a-f0-9]{24})\.[a-f0-9]{24}\./.exec(connection.id);
  if (!match?.[1]) return connection.id;
  const packages = host.plugins.filter(p => p.digest.startsWith(match[1]!));
  return packages.length === 1 ? packages[0]!.name : connection.id;
}
export const agentHref = (id: string, section = "work") => `#agent/${encodeURIComponent(id)}/${section}`;
