// Local design fixtures. No Host or kernel configuration is written from this view.
export type Configuration = {
  name: string; model: string; reasoning: string; maxTokens: number;
  purpose: string; behavior: string; preferences: string; capabilities: string[];
};
export const exampleModels = ["DeepSeek V4.1 Flash", "GLM-5.3-Flash"];
export type CapabilityGroup = {
  kind: "Tools" | "Skills"; via?: "Built-in" | "MCP" | "HTTP API";
  items: { id: string; name: string; detail: string }[];
};
export type CapabilityPlugin = { id: string; name: string; detail: string; connectionId?: string; groups: CapabilityGroup[] };
export const capabilityPlugins: CapabilityPlugin[] = [
  { id: "builtin", name: "Workspace", detail: "Local files and commands · no account needed", groups: [{ kind: "Tools", via: "Built-in", items: [
    { id: "read", name: "Read files", detail: "Read files in the assigned workspace." },
    { id: "search", name: "Search files", detail: "Find text and files across the workspace." },
    { id: "write", name: "Write files", detail: "Create and update workspace files." },
    { id: "shell", name: "Run commands", detail: "Execute commands in the assigned environment." },
  ] }] },
  { id: "mail", connectionId: "mail", name: "AgentMail", detail: "Connection · Personal inbox", groups: [{ kind: "Tools", via: "MCP", items: [
    { id: "mail-read", name: "Read messages", detail: "Read messages and threads in the selected inbox." },
    { id: "mail-send", name: "Send messages", detail: "Send email from the selected inbox." },
  ] }] },
  { id: "github", connectionId: "github", name: "GitHub", detail: "Connection · Personal GitHub", groups: [{ kind: "Tools", via: "MCP", items: [
    { id: "github-read", name: "Read repositories", detail: "Read repository files, issues and pull requests." },
    { id: "github-write", name: "Update repositories", detail: "Create branches, changes and pull requests." },
  ] }] },
  { id: "drive", connectionId: "drive", name: "Google Drive", detail: "Connection · Personal Drive", groups: [{ kind: "Tools", via: "MCP", items: [
    { id: "drive-read", name: "Read documents", detail: "Read and search documents in the selected Drive." },
  ] }] },
  { id: "research", name: "Research & writing", detail: "Public web and reusable methods · no account needed", groups: [
    { kind: "Tools", via: "HTTP API", items: [{ id: "web", name: "Read public web", detail: "Fetch public pages without an account." }] },
    { kind: "Skills", items: [
    { id: "research", name: "Research", detail: "Check original sources and cite the evidence." },
    { id: "writing", name: "Writing", detail: "Organize findings into clear, concise answers." },
    { id: "review", name: "Code review", detail: "Inspect changes and report actionable findings." },
  ] }] },
];
export const pluginCapabilities = (plugin: CapabilityPlugin) => plugin.groups.flatMap(group => group.items);
export function initialConfiguration(name: string): Configuration {
  return { name, model: exampleModels[0]!, reasoning: "Medium", maxTokens: 8192,
    purpose: "Help me manage research, communication and scheduled work. Use the capabilities selected for this agent and report uncertain outcomes clearly.",
    behavior: "Be direct, thoughtful and practical. Keep routine updates concise. Ask when a decision needs my judgment.",
    preferences: "My timezone is Asia/Kolkata. Keep briefings short and link to original sources.",
    capabilities: ["read", "search", "web", "mail-read", "mail-send", "research", "writing"] };
}
export function changedSections(saved: Configuration, draft: Configuration): string[] {
  const changed = [];
  if (["name", "model", "reasoning", "maxTokens"].some(key => saved[key as keyof Configuration] !== draft[key as keyof Configuration])) changed.push("Model & identity");
  if (["purpose", "behavior", "preferences"].some(key => saved[key as keyof Configuration] !== draft[key as keyof Configuration])) changed.push("Instructions");
  if ([...saved.capabilities].sort().join() !== [...draft.capabilities].sort().join()) changed.push("Capabilities");
  return changed;
}
export function setCapabilities(selected: string[], ids: string[], enabled: boolean): string[] {
  return enabled ? [...new Set([...selected, ...ids])] : selected.filter(id => !ids.includes(id));
}
