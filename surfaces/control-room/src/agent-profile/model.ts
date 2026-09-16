// Interactive design data only. Never used to construct a Host runtime.
export type ProfilePart = "setup" | "instructions" | "model" | "tools" | "runtime" | "current" | "inbox" | "brief" | "recent";
export type ProfileScope = "agent" | "inbox" | "brief";
export type CapabilityGroup = "tool" | "skill" | "connection";
export interface Capability { id: string; name: string; detail: string; group: CapabilityGroup; available: boolean }
export interface ProfileDraft {
  name: string;
  role: string;
  model: string;
  reasoning: string;
  maxTokens: number;
  instructions: string;
  soul: string;
  user: string;
  capabilities: string[];
  inboxEnabled: boolean;
  briefEnabled: boolean;
  briefTime: string;
}
export const models = ["DeepSeek V4.1 Flash", "GLM-5.3-Flash"];
export const capabilities: Capability[] = [
  { id: "read_file", name: "Read files", detail: "Read files in the agent’s workspace.", group: "tool", available: true },
  { id: "write_file", name: "Write files", detail: "Create and replace workspace files.", group: "tool", available: true },
  { id: "edit_file", name: "Edit files", detail: "Make targeted changes to existing files.", group: "tool", available: true },
  { id: "bash", name: "Bash", detail: "Run shell commands in the execution environment.", group: "tool", available: true },
  { id: "grep", name: "Search files", detail: "Find text across the workspace.", group: "tool", available: true },
  { id: "research", name: "Research", detail: "Source checking and research instructions.", group: "skill", available: true },
  { id: "writing", name: "Writing", detail: "Clear writing and document preparation.", group: "skill", available: true },
  { id: "review", name: "Code review", detail: "Inspect changes and report actionable findings.", group: "skill", available: true },
  { id: "web", name: "Web search", detail: "Search and read public sources.", group: "connection", available: true },
  { id: "mail", name: "AgentMail", detail: "Personal inbox · Email tools", group: "connection", available: true },
  { id: "drive", name: "Google Drive", detail: "Personal account · Reconnection required", group: "connection", available: false },
  { id: "github", name: "GitHub", detail: "Personal account · Repository tools", group: "connection", available: true },
];
export function initialProfile(): ProfileDraft {
  return {
    name: "Arcee", role: "Personal operator", maxTokens: 8192,
    model: models[0]!, reasoning: "Max",
    instructions: "Help me manage research, communication, and scheduled work. Use the capabilities selected for this agent and report uncertain outcomes clearly.",
    soul: "Be direct, thoughtful, and practical. Keep routine updates concise. Ask when a decision needs my judgment.",
    user: "My timezone is Asia/Kolkata. Keep briefings short and link to the original sources.",
    capabilities: ["read_file", "write_file", "edit_file", "grep", "research", "writing", "web", "mail", "drive"],
    inboxEnabled: true, briefEnabled: false, briefTime: "19:00",
  };
}
export const partTitles: Record<ProfilePart, string> = {
  setup: "Basics", instructions: "Instructions", model: "Model", tools: "Capabilities",
  runtime: "Runtime", current: "Current work", inbox: "Inbox check",
  brief: "Evening brief", recent: "Last completed",
};
export const configurationParts: ProfilePart[] = ["setup", "model", "instructions", "tools"];
export function profileScope(part: ProfilePart): ProfileScope | null {
  return configurationParts.includes(part) ? "agent" : part === "inbox" || part === "brief" ? part : null;
}
const scopedFields: Record<ProfileScope, readonly (keyof ProfileDraft)[]> = {
  agent: ["name", "role", "model", "reasoning", "maxTokens", "instructions", "soul", "user", "capabilities"],
  inbox: ["inboxEnabled"], brief: ["briefEnabled", "briefTime"],
};
// Independent drafts must never overwrite settings saved from another editor.
export function mergeProfileScope(saved: ProfileDraft, draft: ProfileDraft, scope: ProfileScope): ProfileDraft {
  return { ...saved, ...Object.fromEntries(scopedFields[scope].map(key => [key, draft[key]])) };
}
export function hasProfileChanges(saved: ProfileDraft, draft: ProfileDraft, scope: ProfileScope): boolean {
  return scopedFields[scope].some(key => key === "capabilities"
    ? [...saved.capabilities].sort().join("|") !== [...draft.capabilities].sort().join("|")
    : saved[key] !== draft[key]);
}
export const starterPresets = {
  coding: { title: "Coding", description: "Read, edit, search, and run code. Includes the Code review skill and GitHub.", instructions: "Help me understand, implement, and review code. Inspect the relevant sources, make focused changes, and verify the result.", capabilities: ["read_file", "write_file", "edit_file", "grep", "bash", "review", "github"] },
  research: { title: "Research", description: "Read files and search the web. Includes Research and Writing skills.", instructions: "Research the question using original sources. Check uncertain claims, cite useful evidence, and summarize the findings clearly.", capabilities: ["read_file", "grep", "web", "research", "writing"] },
};
export function selectedCapabilities(profile: ProfileDraft, group: CapabilityGroup): Capability[] {
  return capabilities.filter(item => item.group === group && profile.capabilities.includes(item.id));
}
export function capabilitySummary(profile: ProfileDraft): string {
  const parts = [];
  if (profile.capabilities.some(id => ["read_file", "write_file", "edit_file", "grep"].includes(id))) parts.push("Files");
  if (profile.capabilities.includes("bash")) parts.push("shell");
  if (profile.capabilities.includes("web")) parts.push("web");
  if (profile.capabilities.includes("mail")) parts.push("email");
  if (profile.capabilities.includes("github")) parts.push("GitHub");
  if (profile.capabilities.includes("drive")) parts.push("Drive");
  return parts.join(", ") || "No tools selected";
}

// All schedule dates in this design are relative to the labelled 09:42 IST example.
export function nextSchedule(profile: ProfileDraft): { part: "inbox" | "brief"; time: string } | null {
  const [hours, minutes] = profile.briefTime.split(":").map(Number);
  const briefMinute = (hours ?? 0) * 60 + (minutes ?? 0);
  const briefDue = briefMinute <= 9 * 60 + 42 ? briefMinute + 24 * 60 : briefMinute;
  if (profile.inboxEnabled && (!profile.briefEnabled || briefDue >= 600)) return { part: "inbox", time: "10:00 IST" };
  if (profile.briefEnabled) return { part: "brief", time: `${briefDue >= 1440 ? "Tomorrow, " : ""}${profile.briefTime} IST` };
  return null;
}
