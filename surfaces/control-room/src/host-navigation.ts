export type AgentSection = "overview" | "configure" | "activity" | "identity" | "work" | "connections" | "automations" | "policy";
export type HostRoute = { view: "overview" | "agents" | "work" | "library"; agent: string | null; section: AgentSection; execution?: string; tab?: "plugins" | "accounts" };
export function hostRoute(hash: string): HostRoute {
  const [view, id, section, run] = hash.replace(/^#/, "").split("/");
  if (view === "agent" && id) {
    let agent: string;
    try { agent = decodeURIComponent(id); } catch { return { view: "agents", agent: null, section: "work" }; }
    if (section === "activity" && run) {
      try { return { view: "agents", agent, section: "activity", execution: decodeURIComponent(run) }; } catch { return { view: "agents", agent, section: "activity" }; }
    }
    return { view: "agents", agent, section: section === "configure" || section === "activity" || section === "identity" || section === "work" || section === "connections" || section === "automations" || section === "policy" ? section : "overview" };
  }
  if (view === "library") return { view: "library", agent: null, section: "work", tab: id === "accounts" ? "accounts" : "plugins" };
  return { view: view === "agents" || view === "work" ? view : "overview", agent: null, section: "work" };
}

export type AgentPage = "overview" | "configure" | "automations" | "activity";
export function agentPage(section: AgentSection): AgentPage {
  if (section === "identity" || section === "connections") return "configure";
  if (section === "policy") return "automations";
  if (section === "work") return "activity";
  return section;
}
