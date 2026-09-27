import type { HostSnapshot } from "../host-contract";
import { displayName, isEarlier } from "../host-presentation";
import { useSavedPreviewConfiguration } from "../agent-work-preview/configuration-state";
import { usePreviewWork } from "../agent-work-preview/work-state";

export function useDesignAgents(host: HostSnapshot) {
  const saved = useSavedPreviewConfiguration();
  const work = usePreviewWork();
  return host.agents.filter(agent => !isEarlier(agent)).map(agent => {
    const configuration = saved(agent.id, displayName(agent.name));
    return { ...agent, originalName: agent.name, name: configuration.name, configuration, ...work.exampleFor(agent.id) };
  });
}
export type DesignAgent = ReturnType<typeof useDesignAgents>[number];
export { AgentLink, NoResults, PageHeading, agentCount } from "../host-desk";
