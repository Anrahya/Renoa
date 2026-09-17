import type { Execution, ExecutionEvent } from "./data";

// An intentionally demanding design fixture: independent streams on one clock.
export function longExecution(started: number): Execution {
  const events: ExecutionEvent[] = [];
  for (const [lane, count, start, spacing] of [["main", 80, 0, 44], ["worker-a", 60, 300, 28], ["worker-b", 60, 850, 30]] as const) {
    for (let index = 0; index < count; index++) {
      const isModel = index % 2 === 0;
      const name = isModel ? "Model request" : ["sources.read", "workspace.read", "context.lookup"][index % 3]!;
      events.push({
        id: `${lane}-${index}`, lane, offset: start + index * spacing,
        kind: isModel ? "model" : "tool", name, seconds: isModel ? 12 + index % 11 : 3 + index % 7,
        input: isModel ? `Continue with the instructions and recorded results for ${lane}.` : JSON.stringify({ reference: `source-${index + 1}`, scope: "assigned" }, null, 2),
        output: isModel ? "Compared the available evidence and requested the next source. The final response is still pending." : `Read completed. Returned ${2 + index % 6} relevant records.`,
      });
    }
  }
  for (const id of ["worker-a-22", "main-48"]) {
    const event = events.find(item => item.id === id)!;
    event.issue = { code: "rate_limit", origin: "Provider", recovered: true, message: "The provider rate limit was reached. The following request succeeded after a delay." };
    event.output = "HTTP 429 · Rate limited. Retried successfully.";
  }
  const last = events.find(item => item.id === "main-79")!;
  Object.assign(last, { offset: 3540, seconds: 60, kind: "model", name: "Model request", input: "Complete the final recommendation using the recorded findings from all three agents.", output: "HTTP 503 · service_unavailable\nThe provider did not complete the final request. Retry limit reached.", issue: { code: "service_unavailable", origin: "Provider", recovered: false, message: "The final model request failed after its retries. Earlier tool results are retained." } });
  return {
    id: "run-107", title: "Compare the available options", source: "Direct message", started,
    status: "interrupted", input: "Research the available options, compare the evidence, and return a concise recommendation. Delegate independent checks where useful.",
    result: "The final response was interrupted. Earlier findings are saved; the provider could not complete the last request.",
    events, usage: { input: 186420, output: 22480 },
  };
}
