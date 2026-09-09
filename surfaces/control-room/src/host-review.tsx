import { useEffect, useState } from "react";

interface Finding {
  priority?: "P0" | "P1" | "P2" | "P3";
  path: string; line: number; title: string; trigger: string; consequence: string; correction: string;
  evidence: { path: string; start_line: number; quote: string };
}
interface ReviewDetail {
  request_id: string; provider: string | null; model: string | null; reasoning: string | null;
  reason: string | null; report: { findings: Finding[]; limitations: string[] } | null;
}
const record = (value: unknown): value is Record<string, unknown> => typeof value === "object" && value !== null && !Array.isArray(value);
const text = (value: unknown): value is string => typeof value === "string";
const line = (value: unknown): boolean => Number.isSafeInteger(value) && (value as number) > 0;
export function parseReviewDetail(value: unknown, request: string): ReviewDetail {
  if (!record(value) || value.request_id !== request ||
    ![value.provider, value.model, value.reasoning, value.reason].every(v => v === null || text(v)) ||
    !(value.report === null || record(value.report) &&
      Array.isArray(value.report.limitations) && value.report.limitations.every(text) &&
      Array.isArray(value.report.findings) && value.report.findings.every(f => record(f) &&
        (f.priority === undefined || ["P0", "P1", "P2", "P3"].includes(f.priority as string)) &&
        [f.path, f.title, f.trigger, f.consequence, f.correction].every(text) && line(f.line) &&
        record(f.evidence) && text(f.evidence.path) && line(f.evidence.start_line) && text(f.evidence.quote)))) {
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
  return <div className="host-review-evidence">
    {detail.model && <p className="host-caption">{detail.provider} / {detail.model}{detail.reasoning && ` · ${detail.reasoning} reasoning`}</p>}
    {detail.reason && <p className="host-diagnostic">{detail.reason}</p>}
    {detail.report && <>
      <p>{detail.report.findings.length ? `${detail.report.findings.length} recorded findings` : "No findings were recorded."}</p>
      {detail.report.findings.map((finding, index) => <article key={index}>
        <h3>{finding.priority && `${finding.priority} · `}{finding.title}</h3>
        <p className="host-caption"><code>{finding.path}:{finding.line}</code></p>
        <p>{finding.trigger}</p><p>{finding.consequence}</p><p>{finding.correction}</p>
        <details className="host-details"><summary>Supporting evidence</summary><div>
          <p className="host-caption"><code>{finding.evidence.path}:{finding.evidence.start_line}</code></p>
          <pre>{finding.evidence.quote}</pre>
        </div></details>
      </article>)}
      {!!detail.report.limitations.length && <><h3>Review limitations</h3><ul>{detail.report.limitations.map((limit, index) => <li key={index}>{limit}</li>)}</ul></>}
    </>}
    {!detail.reason && !detail.report && <p className="host-secondary">No outcome details are recorded yet.</p>}
  </div>;
}
