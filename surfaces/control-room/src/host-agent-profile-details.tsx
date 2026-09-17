import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { Separator } from "@/components/ui/separator";
import type { Controls } from "./host-controls";
import type { agentOverview } from "./host-agent-overview";
import { ProfileReviewPolicy, ProfileRoutine } from "./host-agent-profile-settings";
import { ProfileReviewRecord, ProfileSessionRecord } from "./host-agent-profile-records";

type Overview = ReturnType<typeof agentOverview>;

export function ProfileAutomations({ data, controls }: { data: Overview; controls: Controls }) {
  return <div className="flex max-w-4xl flex-col gap-8">
    <div className="flex flex-col gap-2"><h2 className="text-xl font-medium">Automations</h2><p className="text-muted-foreground">When this agent runs, and what starts its work.</p></div>
    {controls.preview && <Alert><AlertDescription>Saved preview · Schedule changes and policy saves are disabled. You can explore a policy draft.</AlertDescription></Alert>}
    <section className="profile-settings-section" aria-labelledby="profile-schedules"><div><h3 id="profile-schedules" className="font-medium">Schedules</h3><p className="mt-1 text-sm text-muted-foreground">Timers and recurring work</p></div><div className="flex min-w-0 flex-col gap-5">
      {data.routines.length ? data.routines.map(routine => <ProfileRoutine key={routine.id} {...{ routine, controls }} />) : <p className="text-sm text-muted-foreground">No schedules assigned to this agent.</p>}
    </div></section>
    <Separator />
    <section className="profile-settings-section" aria-labelledby="profile-policy"><div><h3 id="profile-policy" tabIndex={-1} className="scroll-mt-20 font-medium">Repository triggers</h3><p className="mt-1 text-sm text-muted-foreground">Events that start a review</p></div><div className="flex min-w-0 flex-col gap-6">
      {data.repositories.length ? data.repositories.map(repository => <ProfileReviewPolicy key={repository.policy.repository_id} {...{ repository, controls }} />) : <p className="text-sm text-muted-foreground">No repository review policy targets this agent.</p>}
    </div></section>
  </div>;
}

export function ProfileActivity({ data, preview, active }: { data: Overview; preview: boolean; active: boolean }) {
  return <div className="flex max-w-4xl flex-col gap-7">
    <div className="flex flex-col gap-2"><h2 className="text-xl font-medium">Activity</h2><p className="text-muted-foreground">Recorded outcomes and the evidence behind them.</p></div>
    {data.latestReviews.length > 0 && <section aria-labelledby="latest-reviews"><h3 id="latest-reviews" className="font-medium">Latest reviews</h3>{data.latestReviews.map(review => <ProfileReviewRecord key={review.request_id} {...{ review, preview, active }} />)}</section>}
    {data.reviews.length > data.latestReviews.length && <details><summary>Earlier review attempts</summary>{[...data.reviews].reverse().filter(review => !data.latestReviews.includes(review)).map(review => <ProfileReviewRecord key={review.request_id} {...{ review, preview, active }} />)}</details>}
    {data.sessions.length > 0 && <details><summary>Session records <Badge variant="secondary">{data.sessions.length}</Badge></summary>{data.sessions.map(session => <ProfileSessionRecord key={session.id} session={session} />)}</details>}
    {!data.reviews.length && !data.sessions.length && <Empty><EmptyHeader><EmptyTitle>No work recorded</EmptyTitle><EmptyDescription>No recorded work for this agent yet.</EmptyDescription></EmptyHeader></Empty>}
  </div>;
}
