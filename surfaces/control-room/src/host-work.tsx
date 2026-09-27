import { useState } from "react";
import { MagnifyingGlass } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { InputGroup, InputGroupAddon, InputGroupInput } from "@/components/ui/input-group";
import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import { ReviewRow, SessionRow } from "./host-records";
import { PageHeading, NoResults } from "./host-desk";
import { WorkDay } from "./host-work-day";
import { RoutineTimeline } from "./host-routine-timeline";
import { agentName, attentionReviews, sessionNeedsAttention } from "./host-presentation";

export function WorkView({ host, controls }: { host: HostSnapshot; controls: Controls }) {
  const [tab, setTab] = useState("attention");
  const [owner, setOwner] = useState("all");
  const [query, setQuery] = useState("");
  const term = query.trim().toLocaleLowerCase();
  const agents = host.agents.filter(agent => owner === "all" || owner === agent.id);
  const scoped = (id: string | null) => owner === "all" || owner === id;
  const reviews = host.reviews.filter(review => scoped(review.agent_id) && `${review.repository} ${review.pull_number} ${review.state} ${agentName(host, review.agent_id)}`.toLocaleLowerCase().includes(term));
  const sessions = host.sessions.filter(session => scoped(session.agent_id) && `${session.id} ${agentName(host, session.agent_id)}`.toLocaleLowerCase().includes(term));
  const attention = attentionReviews(reviews);
  const interrupted = sessions.filter(sessionNeedsAttention);
  const routines = host.routines.filter(routine => scoped(routine.agent_id));
  return <main id="host-main" className="host-desk">
    <PageHeading title="Work" description="What happened, what needs you, and what starts next." />
    <WorkDay host={host} agents={agents} />
    <Tabs value={tab} onValueChange={value => { setTab(value); setQuery(""); }} className="gap-6">
      <div className="desk-toolbar"><div className="desk-tabs"><TabsList variant="line" aria-label="Work views"><TabsTrigger value="attention">Needs attention <Badge variant="secondary">{attention.length + interrupted.length}</Badge></TabsTrigger><TabsTrigger value="activity">Activity</TabsTrigger><TabsTrigger value="automations">Automations</TabsTrigger></TabsList></div><NativeSelect value={owner} aria-label="Filter work by agent" onChange={event => setOwner(event.target.value)}><NativeSelectOption value="all">All agents</NativeSelectOption>{host.agents.map(agent => <NativeSelectOption key={agent.id} value={agent.id}>{agent.name}</NativeSelectOption>)}</NativeSelect></div>
      <TabsContent value="attention"><div className="desk-section-heading"><h2>Needs your attention</h2><span className="desk-note">Recorded failures and uncertain outcomes</span></div>{attention.map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}{interrupted.map(session => <SessionRow key={session.id} {...{ session, host }} />)}{!attention.length && !interrupted.length && <NoResults title="Nothing needs your attention" />}</TabsContent>
      <TabsContent value="activity"><div className="desk-toolbar"><h2 className="text-base font-medium">Recorded activity</h2><div className="desk-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon><InputGroupInput type="search" aria-label="Search work" placeholder="Search records, outcomes or agents…" value={query} onChange={event => setQuery(event.target.value)} /></InputGroup></div></div>
        {[...reviews].reverse().map(review => <ReviewRow key={review.request_id} {...{ review, host }} preview={controls.preview} />)}{sessions.map(session => <SessionRow key={session.id} {...{ session, host }} />)}{!reviews.length && !sessions.length && <NoResults title={term ? "No matching records" : "No work recorded yet"} clear={term ? () => setQuery("") : undefined} />}
      </TabsContent>
      <TabsContent value="automations"><RoutineTimeline key={owner} {...{ host, controls, routines }} /></TabsContent>
    </Tabs>
  </main>;
}
