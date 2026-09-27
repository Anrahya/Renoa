import { useState } from "react";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { useSavedPreviewConfiguration } from "../agent-work-preview/configuration-state";
import { usePreviewWork } from "../agent-work-preview/work-state";
import type { HostSnapshot } from "../host-contract";
import { displayName, isEarlier } from "../host-presentation";
import { directorySummary } from "../host-agent-directory-model";
import { AgentSpace } from "./agent-map";
import { expandScene, previewManager, previewPlugins } from "./scene-preview";

export function AgentSpacePreview({ host, missingAgent }: { host: HostSnapshot; missingAgent: boolean }) {
  const saved = useSavedPreviewConfiguration();
  const { exampleFor } = usePreviewWork();
  const [expanded, setExpanded] = useState(false);
  const agents = host.agents.filter(agent => !isEarlier(agent));
  const source = agents.map(agent => {
    const configuration = saved(agent.id, displayName(agent.name));
    return { id: agent.id, name: configuration.name, originalName: agent.name, capabilityIds: configuration.capabilities,
      managerId: previewManager(agent, agents), summary: directorySummary(host, agent, exampleFor(agent.id)) };
  });
  return <AgentSpace source={expanded ? expandScene(source) : source} plugins={previewPlugins} missingAgent={missingAgent} selector={
    <NativeSelect aria-label="Example scene" value={expanded ? "50" : "host"} onChange={event => setExpanded(event.target.value === "50")}>
      <NativeSelectOption value="host">Your agents · preview</NativeSelectOption><NativeSelectOption value="50">50-agent example</NativeSelectOption>
    </NativeSelect>} />;
}
