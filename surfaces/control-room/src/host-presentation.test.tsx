import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import type { HostSnapshot, Review, Session } from "./host-contract";
import { agentActivity, attentionReviews, connectionName, currentReviews, sessionNeedsAttention } from "./host-presentation";
import { agentPage, hostRoute } from "./host-navigation";

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
  function render(hash: string, snapshot: HostSnapshot = host, preview = true, search = "") {
    vi.stubGlobal("window", { location: { hash, search } });
    return renderToStaticMarkup(<HostPanelView preview={preview} host={{ snapshot, status: "connected", receivedAt: null, error: null, refresh() {}, lock() {} }} />);
  }
  it("shows directory example telemetry only in the explicit development preview", () => {
    expect(render("#agents", host, true, "?preview")).toContain("Agent relationship map");
    expect(render("#agents", host, false, "?preview")).not.toContain("Agent relationship map");
    expect(render("#agents", host, true)).not.toContain("Agent relationship map");
  });
  it("isolates new workspace fixtures from live and saved Host observations", () => {
    for (const [route, marker] of [["#work", "Today across your agents"], ["#library", "One plugin. Any mix of capabilities."], ["#overview", "Example system condition"]]) {
      expect(render(route!, host, true, "?preview")).toContain(marker);
      expect(render(route!, host, false, "?preview")).not.toContain(marker);
      expect(render(route!, host, true)).not.toContain(marker);
    }
  });
  it("opens work executions only under their agent and handles invalid links", () => {
    expect(render("#work/reviewer/run-107", host, true, "?preview")).toContain("Jump to interruption");
    expect(render("#work/missing/run-107", host, true, "?preview")).not.toContain("Execution timeline");
    expect(hostRoute("#work/%/run-107")).toEqual({ view: "work", agent: null, section: "work" });
    expect(hostRoute("#work/reviewer/%")).toEqual({ view: "work", agent: null, section: "work" });
    expect(hostRoute("#library/accounts").tab).toBe("accounts");
  });
  it("handles direct agent links and malformed locations without breaking the Host", () => {
    expect(hostRoute("#agent/reviewer")).toEqual({ view: "agents", agent: "reviewer", section: "overview" });
    expect(hostRoute("#agent/reviewer/identity")).toEqual({ view: "agents", agent: "reviewer", section: "identity" });
    expect(hostRoute("#agent/reviewer/work")).toEqual({ view: "agents", agent: "reviewer", section: "work" });
    expect(hostRoute("#agent/reviewer/policy")).toEqual({ view: "agents", agent: "reviewer", section: "policy" });
    expect(hostRoute("#agent/%/work")).toEqual({ view: "agents", agent: null, section: "work" });
    const html = render("#agent/missing/work");
    expect(html).toContain("That agent is not in this Host snapshot");
    expect(html).toContain('href="#agent/reviewer/overview"');
  });
  it("deep-links executions with bounded event detail only in the design preview", () => {
    expect(hostRoute("#agent/reviewer/activity/run-107")).toEqual({ view: "agents", agent: "reviewer", section: "activity", execution: "run-107" });
    expect(hostRoute("#agent/reviewer/activity/%")).toEqual({ view: "agents", agent: "reviewer", section: "activity" });
    const html = render("#agent/reviewer/activity/run-107", host, true, "?preview");
    expect(html).toContain("Execution timeline");
    expect(html).toContain("Jump to interruption");
    expect(html).toContain("1–1 of 1");
    expect(html).toContain("service_unavailable");
    expect(html.match(/class="run-event-row/g)).toHaveLength(1);
    expect(render("#agent/reviewer/activity/run-107", host, false, "?preview")).not.toContain("Execution timeline");
    expect(render("#agent/reviewer/activity/run-107", host, true)).not.toContain("Execution timeline");
    expect(render("#agent/reviewer/activity/missing", host, true, "?preview")).toContain("Execution not found");
  });
  it("opens the selected responsibility without dumping every agent section", () => {
    const html = render("#agent/reviewer/connections");
    expect(html).toContain("Selected from the shared library");
    expect(html).not.toContain("No recorded work for this agent yet");
    expect(html).toContain("Automations");
  });
  it("routes every profile destination to an in-page workspace tab", () => {
    for (const section of ["overview", "configure", "automations", "activity"] as const) {
      expect(hostRoute(`#agent/reviewer/${section}`).section).toBe(section);
      const html = render(`#agent/reviewer/${section}`);
      const activeTab = html.match(/<button[^>]*role="tab"[^>]*aria-selected="true"[^>]*>/)?.[0];
      expect(activeTab).toContain(`-trigger-${section}`);
      expect(html).not.toContain("<dialog");
    }
    expect(agentPage("connections")).toBe("configure");
    expect(agentPage("identity")).toBe("configure");
    expect(agentPage("policy")).toBe("automations");
    expect(agentPage("work")).toBe("activity");
  });
  it("keeps unselected Host connections in the agent library without inventing skill assignments", () => {
    const snapshot = { ...host, connections: [
      { id: "selected-mail", catalog_available: true, tool_count: 8, selected_by_profiles: ["review"] },
      { id: "other-drive", catalog_available: true, tool_count: 3, selected_by_profiles: ["other"] },
    ], skills: [{ digest: "abc", name: "private-host-skill" }] };
    const html = render("#agent/reviewer/configure", snapshot);
    expect(html).toContain('aria-label="selected-mail: selected"');
    expect(html).toContain('aria-label="other-drive: not selected"');
    expect(html).toContain("Selection editing is not available yet");
    expect(html).not.toContain("private-host-skill");
    expect(html).not.toContain("Save policy");
  });
  it("preserves live schedule controls while making saved previews read-only", () => {
    vi.stubGlobal("localStorage", { getItem: () => null });
    const snapshot: HostSnapshot = { ...host, routines: [{ id: "r", agent_id: "reviewer", name: "Brief", enabled: false, revision: 1,
      schedule: { kind: "interval", hours: 12 }, next_due_ms: 1, pending_runs: 0, completed_runs: 1 }] };
    const previewSwitch = render("#agent/reviewer/automations", snapshot).match(/<button[^>]*role="switch"[^>]*>/)?.[0];
    const liveSwitch = render("#agent/reviewer/automations", snapshot, false).match(/<button[^>]*role="switch"[^>]*>/)?.[0];
    expect(previewSwitch).toContain('aria-label="Enable schedule: Brief"');
    expect(previewSwitch).toContain('disabled=""');
    expect(liveSwitch).toContain('aria-checked="false"');
    expect(liveSwitch).not.toContain('disabled=""');
  });
  it("opens a factual overview with details progressively disclosed", () => {
    const snapshot: HostSnapshot = { ...host, sessions: [{ id: "private-session-id", agent_id: "reviewer", observation: "unavailable", reason: "Storage unavailable" }] };
    const html = render("#agent/reviewer/overview", snapshot);
    expect(html).toContain("1 session records unavailable");
    expect(html).toContain("Nothing scheduled");
    expect(html).not.toContain("private-session-id");
    expect(html).not.toContain("<dialog");
    expect(html).toContain("Customize");
    expect(html).not.toContain("Max output tokens");
    expect(render("#agent/reviewer/work", snapshot)).toContain("private-session-id");
  });
  it("isolates interactive design fixtures from live and saved Host observations", () => {
    const localDesign = render("#agent/reviewer/automations", host, true, "?preview");
    expect(localDesign).toContain("Example data · 17 Sep, 11:30");
    expect(localDesign).toContain("Morning briefing");
    expect(localDesign).toContain("Timeline range");
    expect(render("#agent/reviewer/automations", host, false, "?preview")).not.toContain("Morning briefing");
    expect(render("#agent/reviewer/automations", host, true)).not.toContain("Morning briefing");
  });
  it("keeps editable configuration fixtures out of live and saved observations", () => {
    const design = render("#agent/reviewer/configure", host, true, "?preview");
    expect(design).toContain("Example configuration");
    expect(design).toContain("Max output tokens");
    expect(design).toContain("Save preview");
    expect(render("#agent/reviewer/configure", host, false, "?preview")).not.toContain("Save preview");
    expect(render("#agent/reviewer/configure", host, true)).not.toContain("Save preview");
  });
  it("keeps stored catalogs distinct from connection health", () => {
    const html = render("#library", { ...host, connections: [{ id: "mail", catalog_available: true, tool_count: 8, selected_by_profiles: ["review"] }] });
    expect(html).toContain("Connection health has not been checked");
    expect(html).toContain('href="#agent/reviewer/connections"');
    expect(html).toContain("Read-only preview");
    expect(html).not.toContain("Host connected");
  });
});
