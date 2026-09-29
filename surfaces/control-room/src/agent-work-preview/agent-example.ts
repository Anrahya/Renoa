import type { Automation, Execution } from "./data";
import { automations, executions } from "./example-data";

export type AgentExample = { automations: Automation[]; executions: Execution[] };
// View fixtures, never agent roles or runtime policy. Sharing this selection
// keeps directory summaries and the records they open on the same agent. The
// Host example spreads one agent across each state so the map, the needs-you
// rail and the state legend can be judged against a realistic mix rather than
// three copies of the same run.
const byId = (ids: string[]) => automations.filter(item => ids.includes(item.id));
const runs = (...ids: string[]) => executions.filter(run => ids.includes(run.id));
export function agentExample(agentId: string): AgentExample {
  if (agentId === "42357f5e-ae1f-0802-5218-d7f65a043086") return {
    automations: byId(["event", "cleanup"]).map(item => ({ ...item, enabled: false })),
    executions: runs("run-105"),
  };
  if (agentId === "c8a63c3b-166d-45a0-9324-2b9db6f3d2df") return {
    automations: byId(["workspace", "event"]),
    executions: runs("run-107", "run-102"),
  };
  // Blocked on the owner: a question the agent asked and has not had answered.
  if (agentId.endsWith("0002")) return { automations: byId(["brief", "event"]), executions: runs("run-106", "run-104") };
  // Broke: a timed-out connection, reported as a failure and not a question.
  if (agentId.endsWith("0003")) return { automations: byId(["workspace"]), executions: runs("run-103", "run-102") };
  // Working on its own, with a paused automation alongside.
  if (agentId.endsWith("0004")) return { automations: byId(["workspace", "cleanup"]), executions: runs("run-102", "run-104") };
  return { automations, executions };
}
