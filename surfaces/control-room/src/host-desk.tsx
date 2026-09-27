import type { ReactNode } from "react";
import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Button } from "@/components/ui/button";
import { agentHref } from "./host-presentation";
import { portraitForAgent } from "./host-identity";
import "./styles/host-design-preview.css";
export function agentCount(count: number) {
  return `${count} ${count === 1 ? "agent" : "agents"}`;
}
export function PageHeading({ title, description, children }: { title: string; description: string; children?: ReactNode }) {
  return <div className="desk-heading"><div><h1 tabIndex={-1}>{title}</h1><p>{description}</p></div>{children}</div>;
}
export function AgentLink({ agent, configure = false }: { agent: { id: string; name: string; originalName?: string }; configure?: boolean }) {
  return <a className="desk-agent-link" href={agentHref(agent.id, configure ? "configure" : "overview")}><img src={portraitForAgent(agent.id, agent.originalName ?? agent.name)} alt="" />{agent.name}</a>;
}
export function NoResults({ clear, title = "Nothing matches this view" }: { clear?: (() => void) | undefined; title?: string }) {
  return <Empty><EmptyHeader><EmptyTitle>{title}</EmptyTitle><EmptyDescription>{clear ? "Try another search or clear the filters." : "New records will appear here when they are available."}</EmptyDescription></EmptyHeader>{clear && <Button variant="outline" size="sm" onClick={clear}>Clear filters</Button>}</Empty>;
}
