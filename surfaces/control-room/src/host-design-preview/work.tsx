import { useEffect, useRef, useState } from "react";
import { ArrowRight, Check, Hourglass, MagnifyingGlass, WarningCircle } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import type { HostSnapshot } from "../host-contract";
import type { HostRoute } from "../host-navigation";
import { agentHref } from "../host-presentation";
import { AutomationTimeline } from "../agent-work-preview/automations";
import { RunPage } from "../agent-work-preview/run-page";
import { usePreviewWork } from "../agent-work-preview/work-state";
import { DAY, TODAY, NOW, date, time, duration, totalSeconds, statusLabel, type Execution } from "../agent-work-preview/data";
import { AgentLink, NoResults, PageHeading, useDesignAgents, type DesignAgent } from "./shared";
import "../styles/agent-work-preview.css";
import "../styles/agent-run-preview.css";
import "../styles/host-work-preview.css";

type WorkRecord = { agent: DesignAgent; run: Execution };
const icons = { completed: Check, interrupted: WarningCircle, waiting: Hourglass };
const key = (agent: string, record: string) => `${agent}/${record}`;
export function WorkPreview({ host, route }: { host: HostSnapshot; route: HostRoute }) {
  const agents = useDesignAgents(host);
  const work = usePreviewWork();
  const [tab, setTab] = useState("attention");
  const [owner, setOwner] = useState("all");
  const [query, setQuery] = useState("");
  const [selectedAutomation, selectAutomation] = useState<string | null>(null);
  const returnTo = useRef<{ id?: string | undefined; scroll: number }>({ scroll: 0 });
  const previous = useRef(route.execution);
  useEffect(() => {
    if (previous.current && !route.execution) requestAnimationFrame(() => {
      window.scrollTo({ top: returnTo.current.scroll });
      const target = [...document.querySelectorAll<HTMLElement>("[data-work-id]")].find(element => element.dataset.workId === returnTo.current.id) ?? document.querySelector<HTMLElement>("#host-main h1");
      target?.focus({ preventScroll: true });
    });
    previous.current = route.execution;
  }, [route.execution]);
  const all = agents.flatMap(agent => agent.executions.map(run => ({ agent, run }))).sort((a, b) => b.run.started - a.run.started);
  const scoped = agents.filter(agent => owner === "all" || owner === agent.id);
  const term = query.trim().toLowerCase();
  const records = all.filter(({ agent, run }) => (owner === "all" || owner === agent.id) && `${agent.name} ${run.title} ${run.result}`.toLowerCase().includes(term));
  const attention = all.filter(({ agent, run }) => (owner === "all" || owner === agent.id) && run.status !== "completed");
  function openRun(agentId: string, runId: string, element: HTMLElement) {
    returnTo.current = { id: element.dataset.workId, scroll: window.scrollY };
    window.location.hash = `#work/${encodeURIComponent(agentId)}/${encodeURIComponent(runId)}`;
  }
  if (route.execution) {
    const item = all.find(({ agent, run }) => agent.id === route.workAgent && run.id === route.execution);
    return <main id="host-main" className="host-desk"><div className="agent-work-preview">{item ? <RunPage key={key(item.agent.id,item.run.id)} run={item.run} agentName={item.agent.name} backLabel="Work" back={() => { window.location.hash = "#work"; }} openAutomation={() => { window.location.hash = agentHref(item.agent.id, "automations"); }} /> : <><NoResults title="This execution isn’t in the preview" /><Button asChild variant="outline"><a href="#work">Back to Work</a></Button></>}</div></main>;
  }
  const automations = scoped.flatMap(agent => agent.automations.map(item => ({ ...item, id: key(agent.id,item.id), name: `${agent.name} / ${item.name}` })));
  const executions = scoped.flatMap(agent => agent.executions.map(run => ({ ...run, id: key(agent.id,run.id), ...(run.automationId ? { automationId: key(agent.id,run.automationId) } : {}) })));
  return <main id="host-main" className="host-desk">
    <PageHeading title="Work" description="What happened, what needs you, and what starts next."><span className="desk-note">{date(TODAY)} · Asia/Kolkata</span></PageHeading>
    <DailyWork agents={scoped} openRun={openRun} />
    <Tabs value={tab} onValueChange={setTab} className="gap-6">
      <div className="desk-toolbar"><div className="desk-tabs"><TabsList variant="line" aria-label="Work views"><TabsTrigger value="attention">Needs attention <Badge variant={attention.length ? "destructive" : "secondary"}>{attention.length}</Badge></TabsTrigger><TabsTrigger value="activity">Activity</TabsTrigger><TabsTrigger value="automations">Automations</TabsTrigger></TabsList></div>
        <NativeSelect value={owner} aria-label="Filter work by agent" onChange={event => { setOwner(event.target.value); selectAutomation(null); }}>{[<NativeSelectOption key="all" value="all">All agents</NativeSelectOption>, ...agents.map(agent => <NativeSelectOption key={agent.id} value={agent.id}>{agent.name}</NativeSelectOption>)]}</NativeSelect>
      </div>
      <TabsContent value="attention"><div className="desk-section-heading"><h2>Needs your attention</h2><span className="desk-note">Interruptions and requests for input</span></div><WorkRows records={attention} openRun={openRun} />{!attention.length && <NoResults title="Nothing needs your attention" />}</TabsContent>
      <TabsContent value="activity"><div className="desk-toolbar"><h2 className="text-base font-medium">Recorded activity</h2><div className="desk-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon><InputGroupInput type="search" aria-label="Search work" placeholder="Search work, outcomes or agents…" value={query} onChange={event => setQuery(event.target.value)} /></InputGroup></div></div>
        {[TODAY, TODAY - DAY].map(day => { const rows = records.filter(({ run }) => run.started >= day && run.started < day + DAY); return rows.length ? <section className="desk-work-day" key={day}><h3>{day === TODAY ? "Today" : "Yesterday"}<span>{date(day)}</span></h3><WorkRows records={rows} openRun={openRun} /></section> : null; })}
        {!records.length && <NoResults clear={() => { setQuery(""); setOwner("all"); }} />}
      </TabsContent>
      <TabsContent value="automations"><div className="agent-work-preview"><AutomationTimeline automations={automations} executions={executions} description="Scheduled and event-driven work across the Host." enabled={Object.fromEntries(automations.map(item => [item.id,item.enabled]))} setEnabled={(id, value) => { const [agent, automation] = id.split("/"); if (agent && automation) work.setEnabled(agent, automation, value); }} selected={selectedAutomation} select={selectAutomation} openRun={(id, element) => { const [agent, run] = id.split("/"); if (agent && run) openRun(agent, run, element); }} /></div></TabsContent>
    </Tabs>
  </main>;
}
function WorkRows({ records, openRun }: { records: WorkRecord[]; openRun: (agent: string, run: string, element: HTMLElement) => void }) {
  return <div className="desk-work-list">{records.map(({ agent, run }) => {
    const Icon = icons[run.status];
    return <article key={key(agent.id,run.id)} className="desk-work-row" data-tone={run.status}>
      <span className="desk-work-icon"><Icon size={19} /></span>
      <div className="desk-work-body"><button className="desk-work-title" data-work-id={`row-${key(agent.id,run.id)}`} onClick={event => openRun(agent.id,run.id,event.currentTarget)}>{run.title}<ArrowRight size={15} /></button><p>{run.result}</p><div className="desk-work-meta"><AgentLink agent={agent} /><span>{run.source}</span><time dateTime={new Date(run.started).toISOString()}>{time(run.started)}</time></div></div>
      <div className="desk-work-outcome"><span>{statusLabel[run.status]}</span><small>{duration(totalSeconds(run))}</small></div>
    </article>;
  })}</div>;
}
function DailyWork({ agents, openRun }: { agents: DesignAgent[]; openRun: (agent: string, run: string, element: HTMLElement) => void }) {
  const today = agents.flatMap(agent => agent.executions.filter(run => run.started >= TODAY && run.started < TODAY + DAY));
  return <section className="desk-day-map" aria-label="Today across your agents"><div className="desk-section-heading"><h2>Today across your agents</h2><span className="desk-note">{today.filter(run => run.status === "completed").length} completed · {today.filter(run => run.status !== "completed").length} need attention</span></div>
    <div className="desk-day-scroll" tabIndex={0} aria-label="Daily work timeline, scroll horizontally on small screens"><div className="desk-day-lanes"><div className="desk-day-axis"><span /><div>{[0,6,12,18,24].map(hour => <span key={hour} style={{ left: `${hour / 24 * 100}%` }}>{hour.toString().padStart(2,"0")}:00</span>)}</div></div>
      {agents.map(agent => <div className="desk-day-lane" key={agent.id}><AgentLink agent={agent} /><div className="desk-day-track"><div className="desk-day-elapsed" style={{ width: `${(NOW - TODAY) / DAY * 100}%` }} /><i className="desk-day-now" style={{ left: `${(NOW - TODAY) / DAY * 100}%` }} />
        {agent.executions.filter(run => run.started >= TODAY && run.started < TODAY + DAY).map(run => <Tooltip key={run.id}><TooltipTrigger asChild><button className="desk-day-mark" data-work-id={`mark-${key(agent.id,run.id)}`} data-tone={run.status} style={{ left: `${(run.started - TODAY) / DAY * 100}%`, width: `${Math.max(.9,totalSeconds(run) * 1000 / DAY * 100)}%` }} aria-label={`${agent.name}: ${run.title}, ${time(run.started)}, ${statusLabel[run.status]}`} onClick={event => openRun(agent.id,run.id,event.currentTarget)} /></TooltipTrigger><TooltipContent>{agent.name} · {run.title}<br />{time(run.started)} · {statusLabel[run.status]}</TooltipContent></Tooltip>)}
      </div></div>)}
      <div className="desk-day-axis"><span /><div><span className="desk-day-now-label" style={{ left: `${(NOW - TODAY) / DAY * 100}%` }}>11:30</span></div></div>
    </div></div><div className="desk-day-legend"><span><i data-tone="completed" />Completed</span><span><i data-tone="waiting" />Needs input</span><span><i data-tone="interrupted" />Interrupted</span><span>Each mark opens an execution</span></div>
  </section>;
}
