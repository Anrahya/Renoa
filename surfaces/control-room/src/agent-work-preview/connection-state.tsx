import { createContext, useContext, useState, type ReactNode } from "react";

// Account access is a separate tab-local example from an agent's selected tools.
export const accountExamples = [
  { id: "mail", name: "Personal inbox", service: "AgentMail", method: "API key", scope: "Read and send email", connected: true },
  { id: "github", name: "Personal GitHub", service: "GitHub", method: "GitHub App", scope: "Selected repositories", connected: true },
  { id: "drive", name: "Personal Drive", service: "Google Drive", method: "OAuth", scope: "Selected documents", connected: false },
  { id: "model", name: "Model access", service: "DeepSeek", method: "API key", scope: "Model requests", connected: true },
];
const initial = Object.fromEntries(accountExamples.map(account => [account.id, account.connected]));
const Connections = createContext<{ connected: Record<string, boolean>; setConnected: (id: string, value: boolean) => void } | null>(null);
export function PreviewConnectionProvider({ children }: { children: ReactNode }) {
  const [connected, set] = useState(initial);
  return <Connections.Provider value={{ connected, setConnected: (id, value) => set(current => ({ ...current, [id]: value })) }}>{children}</Connections.Provider>;
}
export function usePreviewConnections() {
  const context = useContext(Connections);
  if (!context) throw new Error("Connection preview needs its Host provider");
  return context;
}
