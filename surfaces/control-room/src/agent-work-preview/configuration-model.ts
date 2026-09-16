// Local design fixtures. No Host or kernel configuration is written from this view.
export type Configuration = {
  name: string; model: string; reasoning: string; maxTokens: number;
  purpose: string; behavior: string; preferences: string; capabilities: string[];
};
export const exampleModels = ["DeepSeek V4.1 Flash", "GLM-5.3-Flash"];
export const capabilitySources = [
  { id: "builtin", name: "Built-in tools", kind: "Tools", detail: "Workspace and public web · no account needed", items: [
    { id: "read", name: "Read files", detail: "Read files in the assigned workspace." },
    { id: "search", name: "Search files", detail: "Find text and files across the workspace." },
    { id: "write", name: "Write files", detail: "Create and update workspace files." },
    { id: "shell", name: "Run commands", detail: "Execute commands in the assigned environment." },
    { id: "web", name: "Read public web", detail: "Fetch public pages without an account." },
  ] },
  { id: "mail", name: "AgentMail", kind: "MCP", detail: "Connection · Personal inbox", items: [
    { id: "mail-read", name: "Read messages", detail: "Read messages and threads in the selected inbox." },
    { id: "mail-send", name: "Send messages", detail: "Send email from the selected inbox." },
  ] },
  { id: "github", name: "GitHub", kind: "MCP", detail: "Connection · Personal GitHub", items: [
    { id: "github-read", name: "Read repositories", detail: "Read repository files, issues and pull requests." },
    { id: "github-write", name: "Update repositories", detail: "Create branches, changes and pull requests." },
  ] },
  { id: "drive", name: "Google Drive", kind: "MCP", detail: "Connection required · Personal Drive", needsConnection: true, items: [
    { id: "drive-read", name: "Read documents", detail: "Read and search documents in the selected Drive." },
  ] },
  { id: "skills", name: "Skills", kind: "Instructions", detail: "Reusable methods the agent can follow", items: [
    { id: "research", name: "Research", detail: "Check original sources and cite the evidence." },
    { id: "writing", name: "Writing", detail: "Organize findings into clear, concise answers." },
    { id: "review", name: "Code review", detail: "Inspect changes and report actionable findings." },
  ] },
];
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
