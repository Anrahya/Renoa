import { ArrowRight, Clock, CirclesThree, Fingerprint, GitPullRequest, Plugs, Timer } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Card, CardContent, CardDescription, CardFooter, CardHeader, CardTitle } from "@/components/ui/card";
import { Empty, EmptyDescription, EmptyHeader, EmptyMedia, EmptyTitle } from "@/components/ui/empty";
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import { cn } from "@/lib/utils";
import { Separator } from "@/components/ui/separator";
import type { Agent, HostSnapshot } from "./host-contract";
import type { AgentSection } from "./host-navigation";
import { scheduleText, timestamp } from "./host-presentation";
import { agentOverview } from "./host-agent-overview";
import { ProfileReviewSummary } from "./host-agent-profile-records";

export function ProfileOverview({ host, agent, navigate }: { host: HostSnapshot; agent: Agent; navigate: (section: AgentSection) => void }) {
  const data = agentOverview(host, agent);
  const next = data.scheduled[0];
  const activeRepositories = data.repositories.filter(repository => repository.policy.enabled);
  const parts = [
    { panel: "configure" as const, label: "Creation preset", detail: agent.preset_id ?? "No creation preset", icon: Fingerprint },
    { panel: "connections" as const, label: "Connections", detail: `${data.connections.length} selected from the Host`, icon: Plugs },
    { panel: "automations" as const, label: "Automations", detail: `${data.scheduled.length} enabled · ${data.paused.length} paused`, icon: Timer },
    ...(data.repositories.length ? [{ panel: "policy" as const, label: "Review policy", detail: `${data.repositories.length} ${data.repositories.length === 1 ? "repository" : "repositories"}`, icon: GitPullRequest }] : []),
  ];
  return <div className="flex flex-col gap-8">
    <section aria-labelledby="composition-title" className="flex flex-col gap-3">
      <h2 id="composition-title" className="flex items-center gap-2 text-sm font-medium"><CirclesThree className="size-4 text-muted-foreground" />Composition</h2>
      <div className={cn("grid gap-3 sm:grid-cols-2", parts.length === 4 ? "xl:grid-cols-4" : "xl:grid-cols-3")}>
        {parts.map(({ panel: target, label, detail, icon: Icon }) => <Item key={target} asChild variant="outline" size="sm">
          <button onClick={() => navigate(target)}><ItemMedia variant="icon"><Icon /></ItemMedia><ItemContent><ItemTitle>{label}</ItemTitle><ItemDescription>{detail}</ItemDescription></ItemContent><ItemActions><ArrowRight className="size-4" /></ItemActions></button>
        </Item>)}
      </div>
    </section>
    <Separator />
    <div className="grid items-start gap-8 xl:grid-cols-[minmax(0,1.65fr)_minmax(0,1fr)]">
      <section aria-labelledby="work-title" className="flex min-w-0 flex-col gap-5">
        <div className="flex items-center justify-between gap-3"><h2 id="work-title" className="text-base font-medium">Recorded work</h2><Button variant="ghost" size="sm" onClick={() => navigate("activity")}>View records<ArrowRight data-icon="inline-end" /></Button></div>
        {data.activity.tone === "attention" || data.activity.tone === "pending" ? <Item variant="muted" asChild>
          <button onClick={() => navigate("activity")}><ItemContent><ItemTitle>{data.activity.label}</ItemTitle><ItemDescription>{data.unavailableSessions ? `${data.unavailableSessions} session records unavailable` : "Open the records to inspect the latest outcome."}</ItemDescription></ItemContent><ItemActions><ArrowRight className="size-4" /></ItemActions></button>
        </Item> : <p className="text-sm text-muted-foreground">No unfinished work recorded.</p>}
        {data.latestReviews.length ? <div className="flex flex-col gap-1"><p className="mb-2 text-xs text-muted-foreground">Recent review admissions</p>
          {data.latestReviews.slice(0, 3).map(review => <ProfileReviewSummary key={review.request_id} review={review} onOpen={() => navigate("activity")} />)}
        </div> : <Empty className="min-h-40 border"><EmptyHeader><EmptyMedia variant="icon"><Clock /></EmptyMedia><EmptyTitle>{data.sessions.length ? "Work is recorded in sessions" : "No work recorded yet"}</EmptyTitle><EmptyDescription>{data.sessions.length ? `${data.sessions.length} session summaries are available in work records.` : "Reviews and session activity will appear as this agent works."}</EmptyDescription></EmptyHeader></Empty>}
        {data.sessions.length > 0 && <Button variant="outline" className="w-fit" onClick={() => navigate("activity")}>Session records <Badge variant="secondary">{data.sessions.length}</Badge></Button>}
      </section>
      <div className="flex min-w-0 flex-col gap-5">
        <Card><CardHeader><CardTitle>Next up</CardTitle><CardDescription>{next ? "Next scheduled occurrence" : "Scheduled work"}</CardDescription></CardHeader>
          <CardContent className="flex flex-col gap-3"><div className="flex items-start gap-3"><Timer className="mt-0.5 size-5 shrink-0 text-muted-foreground" /><div className="flex min-w-0 flex-col gap-1"><p className="break-words font-medium">{next ? next.name : "Nothing scheduled"}</p><p className="text-xs text-muted-foreground">{next ? timestamp(next.next_due_ms) : data.paused.length ? `${data.paused.length} ${data.paused.length === 1 ? "schedule" : "schedules"} paused` : "No enabled schedules"}</p></div></div>
            {next && <p className="text-xs text-muted-foreground">{scheduleText(next)}</p>}
          </CardContent><CardFooter><Button variant="ghost" size="sm" onClick={() => navigate("automations")}>Manage schedules<ArrowRight data-icon="inline-end" /></Button></CardFooter>
        </Card>
        {data.repositories.length > 0 && <Card><CardHeader><CardTitle>Repository reviews</CardTitle><CardDescription>{activeRepositories.length ? "Triggered by repository events" : "Automatic reviews are paused"}</CardDescription></CardHeader><CardContent className="flex flex-col gap-3">{data.repositories.map(({ policy }) => <div className="flex min-w-0 items-center gap-2" key={policy.repository_id}><GitPullRequest className="size-4 shrink-0 text-muted-foreground" /><span className="truncate text-sm" title={policy.full_name}>{policy.full_name}</span><Badge variant="outline" className="ml-auto">{policy.enabled ? "Enabled" : "Paused"}</Badge></div>)}</CardContent><CardFooter><Button variant="ghost" size="sm" onClick={() => navigate("policy")}>Review settings<ArrowRight data-icon="inline-end" /></Button></CardFooter></Card>}
      </div>
    </div>
  </div>;
}
