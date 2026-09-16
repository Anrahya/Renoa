import { createContext, useContext, useState, type ReactNode } from "react";
import { agentExample } from "./agent-example";

type Overrides = Record<string, Record<string, boolean>>;
const PreviewWork = createContext<{
  exampleFor: typeof agentExample;
  setEnabled: (agentId: string, automationId: string, enabled: boolean) => void;
} | null>(null);

// Tab-local preview edits follow the agent when navigating back to the directory.
export function PreviewWorkProvider({ children }: { children: ReactNode }) {
  const [overrides, setOverrides] = useState<Overrides>({});
  function exampleFor(agentId: string) {
    const example = agentExample(agentId);
    return { ...example, automations: example.automations.map(item => ({ ...item, enabled: overrides[agentId]?.[item.id] ?? item.enabled })) };
  }
  function setEnabled(agentId: string, automationId: string, enabled: boolean) {
    setOverrides(current => ({ ...current, [agentId]: { ...current[agentId], [automationId]: enabled } }));
  }
  return <PreviewWork.Provider value={{ exampleFor, setEnabled }}>{children}</PreviewWork.Provider>;
}
export function usePreviewWork() {
  const context = useContext(PreviewWork);
  if (!context) throw new Error("Work preview needs its Host provider");
  return context;
}
