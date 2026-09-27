import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import { AgentSpace } from "./agent-space/agent-map";
import { CreateAgent } from "./host-agent-create";
import { directorySummary } from "./host-agent-directory-model";
import { connectionName, displayName } from "./host-presentation";
import type { HostRoute } from "./host-navigation";
import { HostAgentProfile } from "./host-agent-profile";
import { workDesignPreview } from "./agent-work-preview/mode";

const AgentSpacePreview = import.meta.env.DEV ? (await import("./agent-space/preview")).AgentSpacePreview : null;

export function AgentsView({ host, route, controls }: { host: HostSnapshot; route: HostRoute; controls: Controls }) {
  const agent = host.agents.find(a => a.id === route.agent);
  if (agent) return <HostAgentProfile key={agent.id} {...{ host, agent, controls }} section={route.section} execution={route.execution} />;
  if (AgentSpacePreview && workDesignPreview(controls.preview)) return <AgentSpacePreview host={host} missingAgent={!!route.agent} />;
  const source = host.agents.map(agent => ({ id: agent.id, name: displayName(agent.name), originalName: agent.name,
    capabilityIds: host.connections.filter(connection => connection.selected_by_agents.includes(agent.id)).map(connection => connection.id),
    summary: directorySummary(host, agent) }));
  const plugins = host.connections.map(connection => ({ id: connection.id, name: connectionName(host, connection), capabilityIds: [connection.id] }));
  return <AgentSpace source={source} plugins={plugins} missingAgent={!!route.agent}><CreateAgent controls={controls} /></AgentSpace>;
}
