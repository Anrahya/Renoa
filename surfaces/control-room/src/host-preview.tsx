import { HostPanelView } from "./host-panel";
import type { HostSnapshot } from "./host-contract";

const id = (n: number) => `00000000-0000-0000-0000-${String(n).padStart(12, "0")}`;
const example: HostSnapshot = {
  host_id: id(1),
  agents: [
    { id: id(2), name: "Arcee", profile: "operator", created_by: null },
    { id: id(3), name: "X Desk", profile: "research", created_by: id(2) },
    { id: id(4), name: "Soundwave", profile: "review", created_by: id(2) },
  ],
  sessions: [],
  routines: [
    { id: id(5), agent_id: id(3), name: "Morning brief", schedule: { kind: "daily", hour: 9, minute: 0, timezone: "Asia/Kolkata" },
      enabled: true, revision: 2, next_due_ms: Date.parse("2026-09-10T09:00:00+05:30"), pending_runs: 0, completed_runs: 3 },
    { id: id(6), agent_id: id(3), name: "Research reminder", schedule: { kind: "once", at: "2026-09-10T14:00:00+05:30" },
      enabled: true, revision: 1, next_due_ms: Date.parse("2026-09-10T14:00:00+05:30"), pending_runs: 0, completed_runs: 0 },
  ],
  connections: [
    { id: "x-api", catalog_available: true, tool_count: 12, selected_by_profiles: ["operator", "research"] },
    { id: "web-search", catalog_available: true, tool_count: 2, selected_by_profiles: ["research"] },
    { id: "github", catalog_available: true, tool_count: 8, selected_by_profiles: ["review"] },
  ],
  plugins: [{ digest: "example-package-revision", name: "Research tools", version: "1.0" }],
  skills: [],
  review_repositories: [{ revision: 2, policy: { repository_id: 42, installation_id: 7, full_name: "Anrahya/Renoa", agent_id: id(4), enabled: true,
    triggers: ["opened", "reopened", "ready_for_review", "synchronize"], skip_drafts: false } }],
  reviews: [{ request_id: id(7), agent_id: id(4), repository: "Anrahya/Renoa", pull_number: 19, admitted_at_ms: Date.parse("2026-09-09T14:00:00Z"),
    reported_head_sha: "23955af".padEnd(40, "0"), reviewed_head_sha: null, state: "prepared", publication: "not_recorded", worker_error: true, retry_after_ms: Date.parse("2026-09-09T14:02:00Z") }],
};
export default function HostPreview() {
  return <HostPanelView preview host={{ status: "connected", snapshot: example, receivedAt: Date.now(), error: null,
    refresh: () => undefined, lock: () => undefined }} />;
}
