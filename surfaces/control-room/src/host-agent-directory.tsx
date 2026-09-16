import { useEffect, useState } from "react";
import { ArrowRight, CirclesThree, MagnifyingGlass, X } from "@phosphor-icons/react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Empty, EmptyContent, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { Alert, AlertDescription } from "@/components/ui/alert";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentOverview } from "./host-agent-overview";
import { agentHref, connectionName, displayName, isEarlier, scheduleText, timestamp } from "./host-presentation";
import { portraitForAgent } from "./host-identity";

type Filter = "all" | "attention" | "automated";

export function HostAgentDirectory({ host, missingAgent = false }: { host: HostSnapshot; missingAgent?: boolean }) {
  const [query, setQuery] = useState("");
  const [filter, setFilter] = useState<Filter>("all");
  const [earlierOpen, setEarlierOpen] = useState(false);
  const agents = host.agents.filter(agent => !isEarlier(agent));
  const earlier = host.agents.filter(isEarlier);
  const summaries = agents.map(agent => ({ agent, overview: agentOverview(host, agent) }));
  const attention = summaries.filter(({ overview }) => overview.activity.tone === "attention").length;
  const automated = summaries.filter(({ overview }) => overview.routines.length > 0 || overview.repositories.length > 0).length;
  const term = query.trim().toLocaleLowerCase();
  useEffect(() => { if (term) setEarlierOpen(true); }, [term]);
  const matches = (agent: Agent) => `${agent.name} ${agent.profile} ${agent.id}`.toLocaleLowerCase().includes(term);
  const visible = summaries.filter(({ agent, overview }) => matches(agent) && (filter === "attention" ? overview.activity.tone === "attention" : filter === "automated" ? overview.routines.length > 0 || overview.repositories.length > 0 : true));
  const visibleEarlier = earlier.filter(matches);
  const reset = () => { setQuery(""); setFilter("all"); };
  return <main id="host-main" className="mx-auto flex w-full max-w-7xl flex-col gap-7 px-4 py-8 md:px-8 md:py-10">
    <div className="flex flex-col gap-2">
      <div className="flex items-center gap-3"><h1 className="text-2xl font-semibold tracking-tight">Agents</h1><Badge variant="secondary">{agents.length}</Badge></div>
      <p className="text-sm text-muted-foreground">Your agents, their work, and what’s coming next.</p>
    </div>
    {missingAgent && <Alert><AlertDescription>That agent is not in this Host snapshot. Choose an available agent below.</AlertDescription></Alert>}
    <Tabs value={filter} onValueChange={value => setFilter(value as Filter)} className="gap-5">
      <div className="flex flex-col-reverse justify-between gap-4 lg:flex-row lg:items-center">
        <TabsList variant="line" aria-label="Filter agents">
          <TabsTrigger value="all" aria-label={`All agents ${agents.length}`}>All <Badge variant="secondary">{agents.length}</Badge></TabsTrigger>
          <TabsTrigger value="attention" aria-label={`Attention ${attention}`}>Attention {attention > 0 && <Badge variant="secondary" className="hidden min-[375px]:inline-flex">{attention}</Badge>}</TabsTrigger>
          <TabsTrigger value="automated" aria-label={`Automated ${automated}`}>Automated {automated > 0 && <Badge variant="secondary" className="hidden min-[375px]:inline-flex">{automated}</Badge>}</TabsTrigger>
        </TabsList>
        <InputGroup className="w-full lg:max-w-64">
          <InputGroupInput type="search" aria-label="Find an agent" placeholder="Search agents…" value={query} onChange={event => setQuery(event.target.value)} />
          <InputGroupAddon><MagnifyingGlass /></InputGroupAddon>
          {query && <InputGroupAddon align="inline-end"><InputGroupButton aria-label="Clear search" size="icon-xs" onClick={() => setQuery("")}><X /></InputGroupButton></InputGroupAddon>}
        </InputGroup>
      </div>
      {(["all", "attention", "automated"] as const).map(value => <TabsContent key={value} value={value} className="flex flex-col gap-4">
        {visible.length > 0 ? <>
          <div aria-hidden="true" className="agent-directory-columns hidden gap-5 border-b px-3 pb-3 text-xs text-muted-foreground min-[1101px]:grid">
            <span>Agent</span><span>Recorded work</span><span>Next automation</span><span>Connections</span>
          </div>
          <ul className="flex flex-col" aria-label="Agents">{visible.map(({ agent, overview }) => <DirectoryRow key={agent.id} {...{ host, agent, overview }} />)}</ul>
        </> : <Empty className="min-h-64 border border-dashed">
          <EmptyHeader><EmptyMedia variant="icon"><CirclesThree /></EmptyMedia>
            <EmptyTitle>{term ? "No matching agents" : value === "attention" ? "No agents need attention" : value === "automated" ? "No automations assigned" : "No named agents yet"}</EmptyTitle>
            <EmptyDescription>{term ? "Try another name, profile, or agent ID." : value === "attention" ? "The Host has no attention records for these agents." : value === "automated" ? "Agents with schedules or repository reviews appear here." : earlier.length ? "Earlier identities are available below." : "Agents will appear here when they are registered on your Host."}</EmptyDescription>
          </EmptyHeader>
          {(term || value !== "all") && <EmptyContent><Button variant="outline" onClick={reset}>Show all agents</Button></EmptyContent>}
        </Empty>}
      </TabsContent>)}
    </Tabs>
    <p role="status" className="text-xs text-muted-foreground">{visible.length} of {agents.length} agents{filter === "automated" ? " · Includes schedules and repository reviews" : " · Status reflects recorded work"}</p>
    {earlier.length > 0 && <details open={earlierOpen} onToggle={event => setEarlierOpen(event.currentTarget.open)} className="border-t pt-5">
      <summary className="text-sm text-muted-foreground">Earlier identities <span className="ml-2">{visibleEarlier.length}</span></summary>
      <p className="mt-3 text-xs text-muted-foreground">Original profile-named identities, retained with their own records.</p>
      <ul className="mt-2 flex flex-col">{visibleEarlier.map(agent => <li key={agent.id} className="border-b py-3 last:border-0"><a href={agentHref(agent.id)} className="flex flex-wrap items-center justify-between gap-2 text-sm hover:underline"><span>{agent.name}</span><code className="text-xs text-muted-foreground">{agent.id.slice(0, 8)}</code><ArrowRight aria-hidden="true" /></a></li>)}</ul>
      {!visibleEarlier.length && <p className="py-3 text-sm text-muted-foreground">No earlier identities match this search.</p>}
    </details>}
  </main>;
}

function DirectoryRow({ host, agent, overview }: { host: HostSnapshot; agent: Agent; overview: ReturnType<typeof agentOverview> }) {
  const next = overview.scheduled[0];
  const review = overview.latestAdmission;
  const activity = overview.activity;
  const state = activity.tone === "attention" ? "Needs attention" : activity.tone === "pending" ? "Unfinished work" : "No unfinished work";
  const enabledRepositories = overview.repositories.filter(repository => repository.policy.enabled);
  const nextText = next ? next.name : enabledRepositories.length ? "On repository events" : "Nothing scheduled";
  const nextDetail = next ? timestamp(next.next_due_ms) : overview.paused.length ? `${overview.paused.length} paused ${overview.paused.length === 1 ? "schedule" : "schedules"}` : enabledRepositories.length ? `${enabledRepositories.length} ${enabledRepositories.length === 1 ? "repository" : "repositories"}` : "—";
  return <li className="agent-directory-columns grid items-start gap-4 border-b px-3 py-5 transition-colors hover:bg-muted/30">
    <a href={agentHref(agent.id)} className="group flex min-w-0 items-center gap-3 rounded-md py-1">
      <Avatar size="lg"><AvatarImage src={portraitForAgent(agent.id, agent.name)} alt="" /><AvatarFallback><CirclesThree aria-hidden="true" /></AvatarFallback></Avatar>
      <span className="truncate font-medium group-hover:underline" title={agent.name}>{displayName(agent.name)}</span>
      <ArrowRight className="ml-auto size-4 shrink-0 text-muted-foreground" aria-hidden="true" />
    </a>
    <div className="flex min-w-0 flex-col items-start gap-2">
      <span className="text-xs text-muted-foreground min-[1101px]:hidden">Recorded work</span>
      <Badge variant={activity.tone === "attention" ? "destructive" : activity.tone === "pending" ? "secondary" : "outline"} asChild><a href={agentHref(agent.id, "work")}>{state}</a></Badge>
      {(activity.tone === "attention" || activity.tone === "pending" || review) && <a href={agentHref(agent.id, "work")} className="max-w-full truncate text-xs text-muted-foreground hover:text-foreground" title={review ? `${review.repository} #${review.pull_number}` : undefined}>{activity.tone === "attention" || activity.tone === "pending" ? activity.label : review ? `${review.repository} #${review.pull_number} · ${review.state}` : ""}</a>}
    </div>
    <div className="flex min-w-0 flex-col gap-1.5">
      <span className="text-xs text-muted-foreground min-[1101px]:hidden">Next automation</span>
      <a href={agentHref(agent.id, next || !enabledRepositories.length ? "automations" : "policy")} className="truncate text-sm hover:underline" title={next ? scheduleText(next) : undefined}>{nextText}</a>
      <span className="text-xs text-muted-foreground">{nextDetail}</span>
    </div>
    <div className="flex min-w-0 flex-col gap-1.5">
      <span className="text-xs text-muted-foreground min-[1101px]:hidden">Connections</span>
      <a href={agentHref(agent.id, "connections")} className="text-sm hover:underline">{overview.connections.length ? `${overview.connections.length} selected` : "None selected"}</a>
      <span className="truncate text-xs text-muted-foreground" title={overview.connections.map(connection => connectionName(host, connection)).join(", ")}>{overview.connections.map(connection => connectionName(host, connection)).join(", ") || "Shared Host library"}</span>
    </div>
  </li>;
}
