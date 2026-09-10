import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import type { HostSnapshot, Review, Session } from "./host-contract";
import { agentActivity, attentionReviews, connectionName, currentReviews, sessionNeedsAttention } from "./host-presentation";
import { hostRoute } from "./host-navigation";

const review = (id: string, fields: Partial<Review> = {}): Review => ({
  request_id: id, agent_id: "reviewer", repository: "owner/repo", pull_number: 1, admitted_at_ms: 1,
  reported_head_sha: "a".repeat(40), reviewed_head_sha: null, publication: "suppressed", worker_error: false,
  retry_after_ms: null, state: "incomplete", ...fields,
});
const host: HostSnapshot = {
  host_id: "host", agents: [{ id: "reviewer", name: "Reviewer", profile: "review", created_by: null }],
  sessions: [], routines: [], connections: [], plugins: [], skills: [], reviews: [], review_repositories: [],
};
afterEach(() => vi.unstubAllGlobals());

describe("Host observation presentation", () => {
  it("uses admission order rather than timestamps and keeps older publication ambiguity visible", () => {
    const old = review("old", { admitted_at_ms: 900 });
    const unknown = review("unknown", { publication: "needs_attention" });
    const newest = review("new", { state: "reviewed", publication: "published" });
    const reviews = [old, unknown, newest];
    expect(currentReviews(reviews)).toEqual([newest]);
    expect(attentionReviews(reviews)).toEqual([unknown]);
    expect(reviews).toEqual([old, unknown, newest]);
  });
  it("retains earlier worker errors with unconfirmed delivery and latest failures", () => {
    const old = review("old", { worker_error: true, publication: "sending" });
    const newest = review("new");
    expect(attentionReviews([old, newest])).toEqual([newest, old]);
  });
  it("describes persisted activity without calling it live", () => {
    const session: Session = { id: "s", agent_id: "reviewer", observation: "available", event_count: 8,
      queued_operations: 0, latest_operation: null,
      active_operation: { id: "op", command_id: "command", position: 1, state: "unfinished" } };
    expect(agentActivity({ ...host, sessions: [session] }, host.agents[0]!)).toEqual({ tone: "pending", label: "1 unfinished record" });
    expect(sessionNeedsAttention({ ...session, active_operation: { ...session.active_operation!, state: "outcome_unknown" } })).toBe(true);
    expect(sessionNeedsAttention({ id: "s", agent_id: "reviewer", observation: "unavailable", reason: "Storage unavailable" })).toBe(true);
  });
  it("maps only unambiguous generated connection IDs to their package names", () => {
    const prefix = "a".repeat(24);
    const connection = { id: `plugin.${prefix}.${"b".repeat(24)}.default`, catalog_available: true, tool_count: 8, selected_by_profiles: [] };
    const plugin = { name: "drive", digest: prefix + "c".repeat(40), version: null };
    expect(connectionName({ ...host, plugins: [plugin] }, connection)).toBe("drive");
    expect(connectionName({ ...host, plugins: [plugin, { ...plugin, digest: prefix + "d".repeat(40) }] }, connection)).toBe(connection.id);
    expect(connectionName(host, { ...connection, id: "my-connection" })).toBe("my-connection");
  });
});

describe("Host navigation and rendered controls", () => {
  function render(hash: string, snapshot: HostSnapshot = host, preview = true) {
    vi.stubGlobal("window", { location: { hash } });
    return renderToStaticMarkup(<HostPanelView preview={preview} host={{ snapshot, status: "connected", receivedAt: null, error: null, refresh() {}, lock() {} }} />);
  }
  it("handles direct agent links and malformed locations without breaking the Host", () => {
    expect(hostRoute("#agent/reviewer/policy")).toEqual({ view: "agents", agent: "reviewer", section: "policy" });
    expect(hostRoute("#agent/%/work")).toEqual({ view: "agents", agent: null, section: "work" });
    const html = render("#agent/missing/work");
    expect(html).toContain("That agent is not in this Host snapshot");
    expect(html).toContain('href="#agent/reviewer/work"');
  });
  it("opens the selected responsibility without dumping every agent section", () => {
    const html = render("#agent/reviewer/connections");
    expect(html).toContain("Selected from the shared library");
    expect(html).not.toContain("No recorded work for this agent yet");
    expect(html).toContain('href="#agent/reviewer/automations"');
  });
  it("preserves live schedule controls while making saved previews read-only", () => {
    const snapshot: HostSnapshot = { ...host, routines: [{ id: "r", agent_id: "reviewer", name: "Brief", enabled: false, revision: 1,
      schedule: { kind: "interval", hours: 12 }, next_due_ms: 1, pending_runs: 0, completed_runs: 1 }] };
    expect(render("#agent/reviewer/automations", snapshot)).toContain('disabled="">Resume schedule');
    expect(render("#agent/reviewer/automations", snapshot, false)).toContain('class="host-action">Resume schedule');
  });
  it("keeps stored catalogs distinct from connection health", () => {
    const html = render("#library", { ...host, connections: [{ id: "mail", catalog_available: true, tool_count: 8, selected_by_profiles: ["review"] }] });
    expect(html).toContain("Connection health has not been checked");
    expect(html).toContain('href="#agent/reviewer/connections"');
    expect(html).toContain("Read-only preview");
    expect(html).not.toContain("Host connected");
  });
});
