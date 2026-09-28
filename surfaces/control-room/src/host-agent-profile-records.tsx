import { Badge } from "@/components/ui/badge";
import type { Session } from "./host-contract";
import { sessionNeedsAttention } from "./host-presentation";

export function ProfileSessionRecord({ session }: { session: Session }) {
  const operation = session.observation === "available" ? session.active_operation ?? session.latest_operation : null;
  const state = session.observation === "unavailable" ? "Records unavailable" : operation?.state.replaceAll("_", " ") ?? "No recorded operation";
  return <details className="border-b py-4 last:border-0"><summary className="text-sm">Session {session.id.slice(0, 8)} <Badge variant={sessionNeedsAttention(session) ? "destructive" : "outline"}>{state}</Badge></summary><div className="mt-4 flex flex-col gap-3 text-xs text-muted-foreground">
    {session.observation === "unavailable" ? <p>{session.reason}</p> : <><p>{session.event_count} recorded events · {session.queued_operations} queued operations</p><p>These are operation records, not the conversation transcript. An unfinished record does not confirm a running worker.</p>{operation && <p>Operation <code>{operation.id}</code></p>}</>}
    <p>Session <code>{session.id}</code></p>
  </div></details>;
}
