import { ArrowRight, Cube, WarningCircle } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { capabilityPlugins, pluginCapabilities } from "../agent-work-preview/configuration-model";
import { agentHref } from "../host-presentation";
import { portraitForAgent } from "../host-identity";
import { pluginMembers, type AgentScene, type PlacedAgent } from "./scene";

export function AgentSelection({ agent, scene, pluginId, onSelect }: { agent: PlacedAgent | undefined; scene: AgentScene; pluginId: string; onSelect: (id: string) => void }) {
  const plugin = capabilityPlugins.find(item => item.id === pluginId);
  if (plugin) {
    const members = pluginMembers(scene.agents, pluginId);
    return <section className="space-selection space-plugin-detail" aria-label={`${plugin.name} shared access`}>
      <div className="space-selection-heading"><Cube size={28} /><div><span className="space-eyebrow">Shared plugin</span><h2>{plugin.name}</h2><p>{plugin.groups.map(group => `${group.kind}${group.via ? ` · ${group.via}` : ""}`).join(" / ")}</p></div></div>
      <div className="space-access"><strong>{members.length} {members.length === 1 ? "agent has" : "agents have"} selected capabilities</strong><p>Each agent keeps its own selection.</p>
        <div className="space-member-buttons">{members.map(member => <button key={member.id} onClick={() => onSelect(member.id)}><img src={portraitForAgent(member.id, member.originalName)} alt="" />{member.name}<span>{member.capabilityIds.filter(id => pluginCapabilities(plugin).some(item => item.id === id)).length}</span></button>)}</div>
        {!members.length && <p>No agents in this scene use this plugin.</p>}
      </div>
    </section>;
  }
  if (!agent) return <section className="space-selection space-selection-empty"><p>Select an agent to see its work and relationships.</p></section>;
  const manager = scene.agents.find(item => item.id === agent.managerId);
  const children = scene.agents.filter(item => item.managerId === agent.id);
  const attention = agent.summary?.tone === "interrupted" || agent.summary?.tone === "waiting";
  return <section className="space-selection" aria-label={`${agent.name} details`}>
    <div className="space-selection-heading"><img className="space-selection-portrait" src={portraitForAgent(agent.id, agent.originalName)} alt="" /><div><span className="space-eyebrow">{agent.synthetic ? "Example agent" : "Selected agent"}</span><h2>{agent.name}</h2>
      <p>{manager ? <>Managed by <button className="space-text-link" onClick={() => onSelect(manager.id)}>{manager.name}</button></> : children.length ? `Manages ${children.length} ${children.length === 1 ? "agent" : "agents"}` : "Independent agent"}</p></div></div>
    {agent.summary && <div className="space-selection-work"><a className="space-work-link" data-attention={attention} href={agent.summary.workHref}>{attention && <WarningCircle size={18} />}<span><small>{attention ? agent.summary.status : "Last recorded work"}</small><strong>{agent.summary.title}</strong></span><ArrowRight size={16} /></a>
      <a className="space-next-link" href={agent.summary.next.href}>{agent.summary.next.title}<ArrowRight size={13} /></a></div>}
    {agent.synthetic ? <p className="space-synthetic-note">Part of the 50-agent example.<br />No Host records attached.</p> : <Button asChild className="space-open"><a href={agentHref(agent.id)}>Open agent<ArrowRight data-icon="inline-end" /></a></Button>}
  </section>;
}
