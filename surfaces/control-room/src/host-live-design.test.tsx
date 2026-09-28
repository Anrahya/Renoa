import { afterEach, describe, expect, it, vi } from "vitest";
import { renderToStaticMarkup } from "react-dom/server";
import { HostPanelView } from "./host-panel";
import type { HostSnapshot } from "./host-contract";
import { portraitForAgent } from "./host-identity";

const agent = { id: "main-agent", name: "Assistant", created_by: null, preset_id: null };
const snapshot: HostSnapshot = { host_id: "host", agents: [agent], sessions: [], automations: [], connections: [], plugins: [], skills: [] };
const render = (hash: string, host = snapshot) => {
  vi.stubGlobal("window", { location: { hash, search: "?preview" } });
  return renderToStaticMarkup(<HostPanelView host={{ snapshot: host, status: "connected", receivedAt: 0, error: null, refresh() {}, lock() {} }} />);
};
afterEach(() => vi.unstubAllGlobals());

describe("Production control-room design", () => {
  it("uses the approved map and page compositions without enabling example data", () => {
    const agents = render("#agents");
    expect(agents).toContain('class="agent-space-page"');
    expect(agents).toContain('aria-label="Agent relationship map"');
    expect(agents).toContain("Create agent");
    expect(agents).not.toContain("50-agent example");
    expect(agents).not.toContain("Example scene");
    for (const [route, marker] of [["#work", "Needs your attention"], ["#library", "Your shared capability library"], ["#overview", "One system, separate responsibilities"]]) {
      const html = render(route!);
      expect(html).toContain('class="host-desk"');
      expect(html).toContain(marker);
      expect(html).not.toContain("host-legacy-page");
      expect(html).not.toContain("Morning briefing");
      expect(html).not.toContain("Example system condition");
      expect(html).not.toContain("22%");
    }
    expect(render("#agent/main-agent/overview")).toContain("At a glance");
    expect(render("#agent/main-agent/automations")).toContain('aria-label="Timeline range"');
  });
  it("uses saved catalog facts without declaring authentication or service health", () => {
    const host = { ...snapshot, connections: [{ id: "public-data", catalog_available: true, tool_count: 30, selected_by_agents: [agent.id] }] };
    const html = render("#library/accounts", host);
    expect(html).toContain("30 tools in stored catalog");
    expect(html).toContain("Authorization unknown");
    expect(html).not.toContain("Unauthorized");
    expect(html).not.toContain("Needs account");
    expect(html).not.toContain("Connect in preview");
    expect(render("#overview", host)).not.toContain("Core services available");
  });
  it("keeps the main agent’s Komi portrait assigned across renames", () => {
    const id = "cb73e0fc-2a17-3072-675b-d0fe73cea853";
    expect(portraitForAgent(id, "Renoa")).toBe("/assets/identities/komi/idle.png");
    expect(portraitForAgent(id, "Renamed")).toBe(portraitForAgent(id, "Renoa"));

  });
});
