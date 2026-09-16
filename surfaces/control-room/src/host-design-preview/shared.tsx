import type { ReactNode } from "react";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Button } from "@/components/ui/button";
import type { HostSnapshot } from "../host-contract";
import { agentHref, displayName, isEarlier } from "../host-presentation";
import { portraitForAgent } from "../host-identity";
import { useSavedPreviewConfiguration } from "../agent-work-preview/configuration-state";
import { usePreviewWork } from "../agent-work-preview/work-state";
import "../styles/host-design-preview.css";

export function useDesignAgents(host: HostSnapshot) {
  const saved = useSavedPreviewConfiguration();
  const work = usePreviewWork();
  return host.agents.filter(agent => !isEarlier(agent)).map(agent => {
    const configuration = saved(agent.id, displayName(agent.name));
    return { ...agent, originalName: agent.name, name: configuration.name, configuration, ...work.exampleFor(agent.id) };
  });
}
export type DesignAgent = ReturnType<typeof useDesignAgents>[number];
export const runHref = (agentId: string, runId: string) => `${agentHref(agentId, "activity")}/${encodeURIComponent(runId)}`;
export function PageHeading({ title, description, children }: { title: string; description: string; children?: ReactNode }) {
  return <div className="desk-heading"><div><h1>{title}</h1><p>{description}</p></div>{children}</div>;
}
export function AgentLink({ agent, configure = false }: { agent: DesignAgent; configure?: boolean }) {
  return <a className="desk-agent-link" href={agentHref(agent.id, configure ? "configure" : "overview")}><img src={portraitForAgent(agent.id, agent.originalName)} alt="" />{agent.name}</a>;
}
export function NoResults({ clear, title = "Nothing matches this view" }: { clear?: () => void; title?: string }) {
  return <Empty><EmptyHeader><EmptyTitle>{title}</EmptyTitle><EmptyDescription>{clear ? "Try another search or clear the filters." : "New records will appear here when they are available."}</EmptyDescription></EmptyHeader>{clear && <Button variant="outline" size="sm" onClick={clear}>Clear filters</Button>}</Empty>;
}
