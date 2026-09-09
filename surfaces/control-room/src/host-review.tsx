import { useEffect, useState } from "react";
import { publicationState, reviewRepository, type PublicationState, type ReviewRepository } from "./host-contract";
import { triggerLabels } from "./host-controls";

interface Finding {
  priority?: "P0" | "P1" | "P2" | "P3";
  side?: "base" | "head"; in_diff?: boolean;
  path: string; line: number; title: string; trigger: string; consequence: string; correction: string;
  evidence: { path: string; start_line: number; quote: string; side?: "base" | "head" };
}
interface ReviewDetail {
  request_id: string; provider: string | null; model: string | null; reasoning: string | null;
  reason: string | null; report: { findings: Finding[]; limitations: string[] } | null;
  repository: ReviewRepository;
  execution: { started_at_ms: number | null; deadline_at_ms: number; retry_after_ms: number; publish_after_ms: number; last_error: string | null } | null;
  publication: { state: PublicationState; reason?: string; url?: string; review_id?: number };
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string";
const line = (value: unknown): boolean => Number.isSafeInteger(value) && (value as number) > 0;
const time = (value: unknown): boolean => Number.isSafeInteger(value) && (value as number) >= 0;
const side = (value: unknown): boolean => value === undefined || value === "base" || value === "head";
function publication(value: unknown): boolean {
  return record(value) && publicationState(value.state) && (value.state === "published" ? line(value.review_id) && text(value.url)
    : ["suppressed", "needs_attention"].includes(value.state) ? text(value.reason) : true);
}
export function parseReviewDetail(value: unknown, request: string): ReviewDetail {
  if (!record(value) || value.request_id !== request ||
    !reviewRepository(value.repository) || !publication(value.publication) ||
    !(value.execution === null || record(value.execution) && (value.execution.started_at_ms === null || time(value.execution.started_at_ms)) &&
      [value.execution.deadline_at_ms, value.execution.retry_after_ms, value.execution.publish_after_ms].every(time) &&
      (value.execution.last_error === null || text(value.execution.last_error))) ||
    ![value.provider, value.model, value.reasoning, value.reason].every(v => v === null || text(v)) ||
    !(value.report === null || record(value.report) &&
      Array.isArray(value.report.limitations) && value.report.limitations.every(text) &&
      Array.isArray(value.report.findings) && value.report.findings.every(f => record(f) &&
        (f.priority === undefined || ["P0", "P1", "P2", "P3"].includes(f.priority as string)) &&
        side(f.side) && (f.in_diff === undefined || typeof f.in_diff === "boolean") &&
        [f.path, f.title, f.trigger, f.consequence, f.correction].every(text) && line(f.line) &&
        record(f.evidence) && side(f.evidence.side) && text(f.evidence.path) && line(f.evidence.start_line) && text(f.evidence.quote)))) {
    throw new Error("The Host returned incompatible review details.");
  }
  return value as unknown as ReviewDetail;
}

/** Fetch the evidence only when its review is expanded; never load frozen review inputs. */
export function ReviewEvidence({ request, state }: { request: string; state: string }) {
  const [detail, setDetail] = useState<ReviewDetail | null>(null);
  const [error, setError] = useState<string | null>(null);
  const [attempt, setAttempt] = useState(0);
  useEffect(() => {
    const controller = new AbortController();
    let disposed = false;
    const timeout = setTimeout(() => controller.abort(), 20_000);
    setDetail(null); setError(null);
    void (async () => {
      try {
        const response = await fetch(`/v1/host/reviews/${encodeURIComponent(request)}`, {
          credentials: "same-origin", cache: "no-store", signal: controller.signal,
        });
        if (!response.ok) throw new Error(response.status === 401 || response.status === 403
          ? "Sign in as this Host’s owner to read review details." : "Review details are temporarily unavailable.");
        const result = parseReviewDetail(await response.json(), request);
        if (!disposed) setDetail(result);
      } catch (error) {
        if (!disposed) setError(error instanceof Error && error.name !== "AbortError" ? error.message : "The request was interrupted. Please retry.");
      } finally { clearTimeout(timeout); }
    })();
    return () => { disposed = true; clearTimeout(timeout); controller.abort(); };
  }, [request, state, attempt]);
  if (error) return <p role="status" className="host-error">{error} <button className="host-link" onClick={() => setAttempt(a => a + 1)}>Retry details</button></p>;
  if (!detail) return <p role="status" className="host-secondary">Loading review details…</p>;
  const date = (value: number) => new Date(value).toLocaleString();
  const url = detail.publication.url;
  const safeUrl = url?.startsWith(`https://github.com/${detail.repository.policy.full_name}/pull/`) ? url : null;
  return <div className="host-review-evidence">
    <button className="host-link" onClick={() => setAttempt(a => a + 1)}>Refresh run details</button>
    {safeUrl && <p><a className="host-link" href={safeUrl} target="_blank" rel="noreferrer">Open published review</a></p>}
    {detail.publication.reason && <p className="host-diagnostic">Publication: {detail.publication.reason}</p>}
    {detail.execution?.last_error && <p className="host-diagnostic host-error">Last worker error: {detail.execution.last_error}</p>}
    {detail.execution && <details className="host-details"><summary>Execution and retry timing</summary><div>
      <p>{detail.execution.started_at_ms === null ? "No worker start recorded." : `Started ${date(detail.execution.started_at_ms)}`}</p>
      <p>Execution deadline · {date(detail.execution.deadline_at_ms)}</p>
      {detail.execution.retry_after_ms > 0 && <p>Worker retry eligible after · {date(detail.execution.retry_after_ms)}</p>}
      {detail.execution.publish_after_ms > 0 && <p>Publication retry eligible after · {date(detail.execution.publish_after_ms)}</p>}
      <p className="host-caption">Recorded timing does not confirm a live worker or guarantee the next attempt.</p>
    </div></details>}
    <details className="host-details"><summary>Policy when this review was admitted · revision {detail.repository.revision}</summary><div>
      <p>{detail.repository.policy.enabled ? "Reviews enabled" : "Reviews disabled"} · {detail.repository.policy.skip_drafts ? "Drafts skipped" : "Drafts included"}</p>
      <p>{detail.repository.policy.triggers.map(t => triggerLabels[t]).join(" · ") || "No automatic triggers selected"}</p>
      <p className="host-caption">This is the captured configuration. The exact triggering event was not retained in this record.</p>
    </div></details>
    {detail.model && <p className="host-caption">{detail.provider} / {detail.model}{detail.reasoning && ` · ${detail.reasoning} reasoning`}</p>}
    {detail.reason && <p className="host-diagnostic">{detail.reason}</p>}
    {detail.report && <>
      <p>{detail.report.findings.length ? `${detail.report.findings.length} recorded findings` : "No findings were recorded."}</p>
      {detail.report.findings.map((finding, index) => <article key={index}>
        <h3>{finding.priority && `${finding.priority} · `}{finding.title}</h3>
        <p className="host-caption"><code>{finding.path}:{finding.line}</code> · {finding.side === "base" ? "Before the change" : "PR head"}{finding.in_diff === false && " · Review body (outside inline diff)"}</p>
        <p>{finding.trigger}</p><p>{finding.consequence}</p><p>{finding.correction}</p>
        <details className="host-details"><summary>Supporting evidence</summary><div>
          <p className="host-caption"><code>{finding.evidence.path}:{finding.evidence.start_line}</code> · {finding.evidence.side === "base" ? "Before the change" : "PR head"}</p>
          <pre>{finding.evidence.quote}</pre>
        </div></details>
      </article>)}
      {!!detail.report.limitations.length && <details className="host-details"><summary>Review limitations · {detail.report.limitations.length}</summary><ul>{detail.report.limitations.map((limit, index) => <li key={index}>{limit}</li>)}</ul></details>}
    </>}
    {!detail.reason && !detail.report && <p className="host-secondary">No outcome details are recorded yet.</p>}
  </div>;
}
