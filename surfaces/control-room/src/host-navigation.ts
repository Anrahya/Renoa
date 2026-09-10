export type AgentSection = "work" | "connections" | "automations" | "policy";
export type HostRoute = { view: "overview" | "agents" | "library"; agent: string | null; section: AgentSection };
export function hostRoute(hash: string): HostRoute {
  const [view, id, section] = hash.replace(/^#/, "").split("/");
  if (view === "agent" && id) {
    let agent: string;
    try { agent = decodeURIComponent(id); } catch { return { view: "agents", agent: null, section: "work" }; }
    return { view: "agents", agent, section: section === "connections" || section === "automations" || section === "policy" ? section : "work" };
  }
  return { view: view === "agents" || view === "library" ? view : "overview", agent: null, section: "work" };
}
