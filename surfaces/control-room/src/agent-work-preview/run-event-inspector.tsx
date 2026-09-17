import { Badge } from "@/components/ui/badge";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { duration } from "./data";
import { issueFor, timecode, type TraceEvent } from "./trace-model";
export function EventInspector({ event, agentName }: { event: TraceEvent | undefined; agentName: string }) {
  if (!event) return <section className="run-event-detail run-no-selection"><p>Select an event to inspect its recorded input and output.</p></section>;
  const issue = issueFor(event);
  return <section className="run-event-detail" aria-label="Selected event" aria-live="polite">
    <div className="flex flex-wrap items-center gap-2"><h3>{event.name}</h3>{issue && <Badge variant={issue.recovered ? "outline" : "destructive"}>{issue.recovered ? "Recovered" : "Interrupted"}</Badge>}</div>
    <p className="work-caption">{event.laneId === "main" ? agentName : event.laneId === "worker-a" ? "Worker A" : "Worker B"} · {timecode(event.start)}–{timecode(event.end)} · {duration(event.seconds)}</p>
    {issue && <div className="run-event-error"><span>{issue.origin}{issue.code && ` · ${issue.code}`}</span><p>{issue.message}</p></div>}
    <Tabs defaultValue="output" key={event.id} className="gap-4"><TabsList variant="line" aria-label="Event payload"><TabsTrigger value="output">Output</TabsTrigger><TabsTrigger value="input">Input</TabsTrigger></TabsList><TabsContent value="output"><pre>{event.output}</pre></TabsContent><TabsContent value="input"><pre>{event.input}</pre></TabsContent></Tabs>
  </section>;
}
