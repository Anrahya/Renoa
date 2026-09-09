export interface Agent { id: string; profile: string; name: string; created_by: string | null }
export type Schedule = { kind: "once"; at: string } | { kind: "interval"; hours: number } |
  { kind: "daily"; hour: number; minute: number; timezone: string };
export interface Routine { id: string; agent_id: string; name: string; schedule: Schedule; enabled: boolean;
  revision: number; next_due_ms: number; pending_runs: number; completed_runs: number }
export interface Operation { id: string; command_id: string; position: number;
  state: "queued" | "unfinished" | "outcome_unknown" | "waiting" | "completed" | "failed" | "cancelled" }
export type Session = { id: string; agent_id: string | null } & (
  { observation: "unavailable"; reason: string } |
  { observation: "available"; event_count: number; queued_operations: number; active_operation: Operation | null; latest_operation: Operation | null });
export interface Connection { id: string; catalog_available: boolean; tool_count: number; selected_by_profiles: string[] }
export interface Plugin { digest: string; name: string; version: string | null }
export interface Skill { digest: string; name: string }
export interface Review { request_id: string; agent_id: string; repository: string; pull_number: number;
  admitted_at_ms: number; reported_head_sha: string; reviewed_head_sha: string | null;
  state: "queued" | "prepared" | "reviewed" | "skipped" | "incomplete" | "superseded" }
export interface HostSnapshot { host_id: string; agents: Agent[]; sessions: Session[]; routines: Routine[];
  connections: Connection[]; plugins: Plugin[]; skills: Skill[]; reviews: Review[] }

type RecordValue = Record<string, unknown>;
const record = (v: unknown): v is RecordValue => typeof v === "object" && v !== null && !Array.isArray(v);
const text = (v: unknown): v is string => typeof v === "string";
const id = (v: unknown): v is string => text(v) && /^[\da-f]{8}-[\da-f]{4}-[\da-f]{4}-[\da-f]{4}-[\da-f]{12}$/i.test(v);
const count = (v: unknown): v is number => Number.isSafeInteger(v) && (v as number) >= 0;
const array = (v: unknown, check: (v: unknown) => boolean): boolean => Array.isArray(v) && v.every(check);
const nullable = (v: unknown, check: (v: unknown) => boolean): boolean => v === null || check(v);
function operation(v: unknown): boolean {
  return nullable(v, v => record(v) && id(v.id) && id(v.command_id) && count(v.position) &&
    ["queued", "unfinished", "outcome_unknown", "waiting", "completed", "failed", "cancelled"].includes(v.state as string));
}
function schedule(v: unknown): boolean {
  if (!record(v)) return false;
  if (v.kind === "once") return text(v.at) && Number.isFinite(Date.parse(v.at));
  if (v.kind === "interval") return count(v.hours) && v.hours > 0;
  return v.kind === "daily" && count(v.hour) && v.hour < 24 && count(v.minute) && v.minute < 60 && text(v.timezone);
}
export function parseHost(value: unknown): HostSnapshot {
  if (!record(value) || !id(value.host_id) ||
    !array(value.agents, v => record(v) && id(v.id) && text(v.name) && text(v.profile) && nullable(v.created_by, id)) ||
    !array(value.sessions, v => record(v) && id(v.id) && nullable(v.agent_id, id) && (
      v.observation === "unavailable" ? text(v.reason) : v.observation === "available" && count(v.event_count) &&
      count(v.queued_operations) && operation(v.active_operation) && operation(v.latest_operation))) ||
    !array(value.routines, v => record(v) && id(v.id) && id(v.agent_id) && text(v.name) && schedule(v.schedule) &&
      typeof v.enabled === "boolean" && count(v.revision) && count(v.next_due_ms) && count(v.pending_runs) && count(v.completed_runs)) ||
    !array(value.connections, v => record(v) && text(v.id) && typeof v.catalog_available === "boolean" && count(v.tool_count) && array(v.selected_by_profiles, text)) ||
    !array(value.plugins, v => record(v) && text(v.digest) && text(v.name) && nullable(v.version, text)) ||
    !array(value.skills, v => record(v) && text(v.digest) && text(v.name)) ||
    !array(value.reviews, v => record(v) && id(v.request_id) && id(v.agent_id) && text(v.repository) && count(v.pull_number) &&
      count(v.admitted_at_ms) && text(v.reported_head_sha) && nullable(v.reviewed_head_sha, text) &&
      ["queued", "prepared", "reviewed", "skipped", "incomplete", "superseded"].includes(v.state as string))) {
    throw new Error("The Host returned an incompatible snapshot. Your last received state is preserved.");
  }
  return value as unknown as HostSnapshot;
}
