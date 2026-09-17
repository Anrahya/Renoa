import { useState } from "react";
import { ArrowRight, ArrowUpRight, GitPullRequest } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Item, ItemActions, ItemContent, ItemDescription, ItemMedia, ItemTitle } from "@/components/ui/item";
import type { Review, Session } from "./host-contract";
import { needsAttention, sessionNeedsAttention, timestamp } from "./host-presentation";
import { ReviewEvidence } from "./host-review";

function reviewStatus(review: Review) {
  return review.worker_error && ["not_recorded", "sending"].includes(review.publication) ? "Worker needs attention" :
    { queued: "Queued", prepared: "Prepared", reviewed: "Review complete", skipped: "Skipped", incomplete: "Incomplete", superseded: "Superseded" }[review.state];
}
const delivery = { not_recorded: "No publication recorded", sending: "Publication unconfirmed", published: "Published to GitHub", suppressed: "Publication suppressed", needs_attention: "Publication needs attention" };

export function ProfileReviewSummary({ review, onOpen }: { review: Review; onOpen: () => void }) {
  return <Item asChild><button onClick={onOpen}>
    <ItemMedia variant="icon"><GitPullRequest /></ItemMedia><ItemContent><ItemTitle>{review.repository} #{review.pull_number}</ItemTitle><ItemDescription>{timestamp(review.admitted_at_ms)} · {delivery[review.publication]}</ItemDescription></ItemContent>
    <ItemActions><Badge variant={needsAttention(review) ? "destructive" : "outline"}>{reviewStatus(review)}</Badge><ArrowRight className="size-4" /></ItemActions>
  </button></Item>;
}

export function ProfileReviewRecord({ review, preview, active }: { review: Review; preview: boolean; active: boolean }) {
  const [expanded, setExpanded] = useState(false);
  const url = `https://github.com/${review.repository.split("/").map(encodeURIComponent).join("/")}/pull/${review.pull_number}`;
  return <details className="border-b py-4 last:border-0" onToggle={event => setExpanded(event.currentTarget.open)}>
    <summary className="text-sm font-medium"><span className="break-words">{review.repository} #{review.pull_number}</span><Badge className="ml-2" variant={needsAttention(review) ? "destructive" : "outline"}>{reviewStatus(review)}</Badge><span className="mt-1 block text-xs font-normal text-muted-foreground">{timestamp(review.admitted_at_ms)} · {delivery[review.publication]}</span></summary>
    <div className="mt-4 flex flex-col gap-3 text-sm"><dl className="flex flex-col gap-3 text-xs"><div><dt className="text-muted-foreground">Requested commit</dt><dd><code>{review.reported_head_sha}</code></dd></div>{review.reviewed_head_sha && <div><dt className="text-muted-foreground">Reviewed commit</dt><dd><code>{review.reviewed_head_sha}</code></dd></div>}</dl>
      <p className="text-xs text-muted-foreground">{review.state === "reviewed" ? `${delivery[review.publication]}. A completed review is not an approval or a test result.` : review.state === "incomplete" ? "This review did not complete. Its diagnostics stay here in your Host." : "This is the recorded review state; worker liveness is not confirmed."}</p>
      {expanded && active && (preview ? <p className="text-xs text-muted-foreground">This preview contains summary records. Full findings and execution evidence are available in the live Host.</p> : <div className="host-app profile-evidence"><ReviewEvidence request={review.request_id} state={`${review.state}:${review.publication}:${review.retry_after_ms}:${review.worker_error}`} /></div>)}
      <Button variant="outline" size="sm" className="w-fit" asChild><a href={url} target="_blank" rel="noreferrer">Open pull request<ArrowUpRight data-icon="inline-end" /></a></Button>
    </div>
  </details>;
}

export function ProfileSessionRecord({ session }: { session: Session }) {
  const operation = session.observation === "available" ? session.active_operation ?? session.latest_operation : null;
  const state = session.observation === "unavailable" ? "Records unavailable" : operation?.state.replaceAll("_", " ") ?? "No recorded operation";
  return <details className="border-b py-4 last:border-0"><summary className="text-sm">Session {session.id.slice(0, 8)} <Badge variant={sessionNeedsAttention(session) ? "destructive" : "outline"}>{state}</Badge></summary><div className="mt-4 flex flex-col gap-3 text-xs text-muted-foreground">
    {session.observation === "unavailable" ? <p>{session.reason}</p> : <><p>{session.event_count} recorded events · {session.queued_operations} queued operations</p><p>These are operation records, not the conversation transcript. An unfinished record does not confirm a running worker.</p>{operation && <p>Operation <code>{operation.id}</code></p>}</>}
    <p>Session <code>{session.id}</code></p>
  </div></details>;
}
