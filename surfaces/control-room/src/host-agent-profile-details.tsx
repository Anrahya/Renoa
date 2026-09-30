import { Empty, EmptyDescription, EmptyHeader, EmptyTitle } from "@/components/ui/empty";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { Badge } from "@/components/ui/badge";
import { AutomationTimeline } from "./host-automation-timeline";
import type { HostSnapshot } from "./host-contract";
import type { Controls } from "./host-controls";
import type { agentOverview } from "./host-agent-overview";
import { ProfileSessionRecord } from "./host-agent-profile-records";

type Overview = ReturnType<typeof agentOverview>;

export function ProfileAutomations({ data, controls, host }: { data: Overview; controls: Controls; host: HostSnapshot }) {
  return <div className="flex max-w-4xl flex-col gap-8">
    {controls.preview && <Alert><AlertDescription>Saved preview · Schedule changes are disabled.</AlertDescription></Alert>}
    <AutomationTimeline automations={data.automations} host={host} controls={controls} />
  </div>;
}

export function ProfileActivity({ data }: { data: Overview }) {
  return <div className="flex max-w-4xl flex-col gap-7">
    <div className="flex flex-col gap-2"><h2 className="text-xl font-medium">Activity</h2><p className="text-muted-foreground">Recorded outcomes and the evidence behind them.</p></div>
    {data.sessions.length > 0 && <details><summary>Session records <Badge variant="secondary">{data.sessions.length}</Badge></summary>{data.sessions.map(session => <ProfileSessionRecord key={session.id} session={session} />)}</details>}
    {!data.sessions.length && <Empty><EmptyHeader><EmptyTitle>No work recorded</EmptyTitle><EmptyDescription>No recorded work for this agent yet.</EmptyDescription></EmptyHeader></Empty>}
  </div>;
}
