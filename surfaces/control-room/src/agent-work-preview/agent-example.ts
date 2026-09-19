import type { Automation, Execution } from "./data";
import { automations, executions } from "./example-data";

export type AgentExample = { automations: Automation[]; executions: Execution[] };
// View fixtures, never agent roles or runtime policy. Sharing this selection
// keeps directory summaries and the records they open on the same agent.
export function agentExample(agentId: string): AgentExample {
  if (agentId === "42357f5e-ae1f-0802-5218-d7f65a043086") return {
    automations: automations.filter(item => item.id === "event" || item.id === "cleanup").map(item => ({ ...item, enabled: false })),
    executions: executions.filter(run => run.id === "run-105"),
  };
  if (agentId === "c8a63c3b-166d-45a0-9324-2b9db6f3d2df") return {
    automations: automations.filter(item => item.id === "workspace" || item.id === "event"),
    executions: executions.filter(run => run.id === "run-107" || run.id === "run-102"),
  };
  return { automations, executions };
}
