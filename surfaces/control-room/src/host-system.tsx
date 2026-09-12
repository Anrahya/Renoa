import type { HostSnapshot } from "./host-contract";
import { AgentMap } from "./host-agent-map";

export function SystemView({ host, live, receivedAt }: { host: HostSnapshot; live: boolean; receivedAt: number | null }) {
  return <main id="host-main" className="host-content host-system-page"><h1>System</h1>
    <AgentMap {...{ host, live, receivedAt }} />
  </main>;
}
