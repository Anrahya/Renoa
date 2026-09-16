import { useEffect, useState } from "react";
import { ArrowRight, CaretRight, CirclesThree, Clock, MagnifyingGlass, WarningCircle, X } from "@phosphor-icons/react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Item, ItemContent, ItemGroup, ItemMedia } from "@/components/ui/item";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Tooltip, TooltipContent, TooltipTrigger } from "@/components/ui/tooltip";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentHref, displayName, isEarlier } from "./host-presentation";
import { portraitForAgent } from "./host-identity";
import { directorySummary, type DirectorySummary } from "./host-agent-directory-model";
import { workDesignPreview } from "./agent-work-preview/mode";
import { usePreviewWork } from "./agent-work-preview/work-state";
import { useSavedPreviewConfiguration } from "./agent-work-preview/configuration-state";
import { cn } from "@/lib/utils";

type Filter = "all" | "attention" | "automated";
const needsAttention = (summary: DirectorySummary) => summary.tone === "interrupted" || summary.tone === "waiting";

export function HostAgentDirectory({ host, preview = false, missingAgent = false }: { host: HostSnapshot; preview?: boolean; missingAgent?: boolean }) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [earlierOpen, setEarlierOpen] = useState(false);
  const designPreview = workDesignPreview(preview);
  const { exampleFor } = usePreviewWork();
  const savedConfiguration = useSavedPreviewConfiguration();
  const earlier = host.agents.filter(isEarlier);
  const summaries = host.agents.filter(agent => !isEarlier(agent)).map(agent => {
    const configuration = designPreview ? savedConfiguration(agent.id, displayName(agent.name)) : null;
    return { agent, name: configuration?.name ?? displayName(agent.name),
      subtitle: configuration?.model ?? "Host-owned agent",
      summary: directorySummary(host, agent, designPreview ? exampleFor(agent.id) : undefined),
    };
  });
  const attention = summaries.filter(({ summary }) => needsAttention(summary)).length;
  const automated = summaries.filter(({ summary }) => summary.automated).length;
  const term = query.trim().toLocaleLowerCase();
  useEffect(() => { if (term) setEarlierOpen(true); }, [term]);
  const matches = (agent: Agent, name = agent.name) => `${name} ${agent.name} ${agent.profile} ${agent.id}`.toLocaleLowerCase().includes(term);
  const visible = summaries.filter(({ agent, name, summary }) => matches(agent, name) && (filter === "attention" ? needsAttention(summary) : filter === "automated" ? summary.automated : true));
  const visibleEarlier = earlier.filter(agent => matches(agent));
  const reset = () => { setQuery(""); setFilter("all"); };
  return <main id="host-main" className="agent-directory mx-auto flex w-full max-w-6xl flex-col gap-7 px-4 py-8 md:px-8 md:py-10">
    <div className="directory-heading">
      <div className="flex flex-col gap-2"><div className="flex items-center gap-3"><h1 className="text-2xl font-semibold tracking-tight">Agents</h1><Badge variant="secondary">{summaries.length}</Badge></div>
        <p className="text-sm text-muted-foreground">See what needs you and what happens next.</p></div>
      {designPreview && <span className="directory-date">17 September <span>Today · Asia/Kolkata</span></span>}
    </div>
    {missingAgent && <Alert><AlertDescription>That agent is not in this Host snapshot. Choose an available agent below.</AlertDescription></Alert>}
    <Tabs value={filter} onValueChange={value => setFilter(value as Filter)} className="gap-2">
      <div className="flex flex-col-reverse justify-between gap-4 pb-4 lg:flex-row lg:items-center">
        <TabsList variant="line" aria-label="Filter agents">
          <TabsTrigger value="all" aria-label={`All agents ${summaries.length}`}>All <Badge variant="secondary">{summaries.length}</Badge></TabsTrigger>
          <TabsTrigger value="attention" aria-label={`Attention ${attention}`}>Attention {attention > 0 && <Badge variant="secondary">{attention}</Badge>}</TabsTrigger>
          <TabsTrigger value="automated" aria-label={`Automated ${automated}`}>Automated</TabsTrigger>
        </TabsList>
        <InputGroup className="w-full lg:max-w-64">
          <InputGroupInput type="search" aria-label="Find an agent" placeholder="Find an agent…" value={query} onChange={event => setQuery(event.target.value)} />
          <InputGroupAddon><MagnifyingGlass /></InputGroupAddon>
          {query && <InputGroupAddon align="inline-end"><InputGroupButton aria-label="Clear search" size="icon-xs" onClick={() => setQuery("")}><X /></InputGroupButton></InputGroupAddon>}
        </InputGroup>
      </div>
      <TabsContent value={filter}>
        {visible.length > 0 ? <ItemGroup aria-label="Agents" className="gap-0">{visible.map(({ agent, name, subtitle, summary }) => <DirectoryRow key={agent.id} {...{ agent, name, subtitle, summary }} />)}</ItemGroup> : <Empty className="min-h-64 border border-dashed">
          <EmptyHeader><EmptyMedia variant="icon"><CirclesThree /></EmptyMedia>
            <EmptyTitle>{term ? "No matching agents" : filter === "attention" ? "No agents need attention" : filter === "automated" ? "No automations assigned" : "No named agents yet"}</EmptyTitle>
            <EmptyDescription>{term ? "Try another name, profile, or agent ID." : filter === "attention" ? "There are no attention records for these agents." : filter === "automated" ? "Agents with schedules or event triggers appear here, including paused ones." : earlier.length ? "Earlier identities are available below." : "Agents will appear here when they are registered on your Host."}</EmptyDescription>
          </EmptyHeader>
          {(term || filter !== "all") && <EmptyContent><Button variant="outline" onClick={reset}>Show all agents</Button></EmptyContent>}
        </Empty>}
      </TabsContent>
    </Tabs>
    <div className="directory-footer"><p role="status">{visible.length} of {summaries.length} agents · {designPreview ? "Example activity" : "Status reflects recorded work"}</p>
      {designPreview && <div className="directory-legend"><span><i data-tone="completed" />Completed</span><span><i data-tone="waiting" />Needs input</span><span><i data-tone="interrupted" />Interrupted</span></div>}
    </div>
    {earlier.length > 0 && <details open={earlierOpen} onToggle={event => setEarlierOpen(event.currentTarget.open)} className="border-t pt-5">
      <summary className="text-sm text-muted-foreground">Earlier identities <span className="ml-2">{visibleEarlier.length}</span></summary>
      <p className="mt-3 text-sm text-muted-foreground">Original profile-named identities, retained with their own records.</p>
      <ul className="mt-2 flex flex-col">{visibleEarlier.map(agent => <li key={agent.id} className="border-b py-3 last:border-0"><a href={agentHref(agent.id)} className="flex flex-wrap items-center justify-between gap-2 text-sm hover:underline"><span className="break-all">{agent.name}</span><code className="text-xs text-muted-foreground">{agent.id.slice(0, 8)}</code><ArrowRight aria-hidden="true" /></a></li>)}</ul>
      {!visibleEarlier.length && <p className="py-3 text-sm text-muted-foreground">No earlier identities match this search.</p>}
    </details>}
  </main>;
}

function DirectoryRow({ agent, name, subtitle, summary }: { agent: Agent; name: string; subtitle: string; summary: DirectorySummary }) {
  return <Item role="listitem" className="directory-agent" data-tone={summary.tone}>
    <ItemMedia className="directory-identity"><a href={agentHref(agent.id)} className="directory-agent-link">
      <Avatar size="lg"><AvatarImage src={portraitForAgent(agent.id, agent.name)} alt="" /><AvatarFallback><CirclesThree aria-hidden="true" /></AvatarFallback></Avatar>
      <span><h2>{name}</h2><span className="directory-profile" title={agent.profile}>{subtitle}</span></span>
      <CaretRight aria-hidden="true" />
    </a></ItemMedia>
    <ItemContent className="directory-work">
      <div className="directory-status"><span className="directory-state" data-tone={summary.tone}>{summary.tone === "interrupted" ? <WarningCircle size={15} aria-hidden="true" /> : <i aria-hidden="true" />}{summary.status}</span><a href={agentHref(agent.id, "configure")} className="directory-configure">Configure<ArrowRight size={12} aria-hidden="true" /></a></div>
      <a className="directory-work-link" href={summary.workHref}><strong>{summary.title}</strong><p>{summary.detail}</p></a>
      <a className="directory-next" href={summary.next.href}><Clock size={16} aria-hidden="true" /><span><strong>{summary.next.title}</strong><small>{summary.next.detail}</small></span><CaretRight size={14} aria-hidden="true" /></a>
    </ItemContent>
    <div className="directory-day">{summary.day ? <DayStrip name={name} agentId={agent.id} day={summary.day} /> : <a href={agentHref(agent.id, "automations")} className="directory-observation"><span>Automations</span><strong>{summary.automationCount}</strong><span>Schedules & event triggers</span></a>}</div>
  </Item>;
}

function DayStrip({ name, agentId, day }: { name: string; agentId: string; day: NonNullable<DirectorySummary["day"]> }) {
  const label = `${name} today: ${day.total} ${day.total === 1 ? "run" : "runs"}, ${day.completed} completed, ${day.attention} ${day.attention === 1 ? "needs" : "need"} attention. Open activity`;
  return <Tooltip delayDuration={150}><TooltipTrigger asChild><a className="directory-day-link" href={agentHref(agentId, "activity")} aria-label={label}>
    <span className="directory-day-heading"><strong>Today’s work</strong><span>{day.total} {day.total === 1 ? "run" : "runs"}<ArrowRight size={12} /></span></span>
    <span className="directory-hour-strip" aria-hidden="true">{day.bins.map((tone, hour) => <i key={hour} data-tone={tone} className={cn(hour >= 12 && "directory-hour-future")} />)}<span className="directory-now" style={{ left: `${11.5 / 24 * 100}%` }} /></span>
    <span className="directory-hours" aria-hidden="true"><span>00</span><span>06</span><span>12</span><span>18</span><span>24</span></span>
    <span className="directory-outcomes">{day.completed} completed{day.attention > 0 && <span> · {day.attention} {day.attention === 1 ? "needs" : "need"} attention</span>}</span>
  </a></TooltipTrigger><TooltipContent className="flex max-w-72 flex-col gap-1"><strong>{day.completed} completed · {day.attention} {day.attention === 1 ? "needs" : "need"} attention</strong><span>Runs are grouped by their start hour. Interruptions stay visible when outcomes overlap.</span><span>Observation: 11:30 · Open activity for individual runs.</span></TooltipContent></Tooltip>;
}
