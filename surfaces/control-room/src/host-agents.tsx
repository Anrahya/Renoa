import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import { HostAgentDirectory } from "./host-agent-directory";
import type { HostRoute } from "./host-navigation";
import { HostAgentProfile } from "./host-agent-profile";

export function AgentsView({ host, route, controls }: { host: HostSnapshot; route: HostRoute; controls: Controls }) {
  const agent = host.agents.find(a => a.id === route.agent);
  if (agent) return <HostAgentProfile key={agent.id} {...{ host, agent, controls }} section={route.section} execution={route.execution} />;
  return <HostAgentDirectory host={host} preview={controls.preview} missingAgent={!!route.agent} />;
}
