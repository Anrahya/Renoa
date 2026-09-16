import { CaretRight, Check, WarningCircle, Hourglass } from "@phosphor-icons/react";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { cn } from "@/lib/utils";
import { date, duration, executions, statusLabel, time, TODAY, totalSeconds, type Execution } from "./data";
const statusIcons = { completed: Check, interrupted: WarningCircle, waiting: Hourglass };
export function ActivityExplorer({ openRun, filter, setFilter }: { openRun: (id: string, element: HTMLElement) => void; filter: string; setFilter: (value: string) => void }) {
  const visible = executions.filter(run => filter === "all" || (filter === "attention" ? run.status !== "completed" : run.status === "completed")).sort((a, b) => b.started - a.started);
  return <div className="flex flex-col gap-6">
    <div className="work-heading"><div><h2>Activity</h2><p>The work, its outcome, and the record behind it.</p></div><span className="work-caption">Last message received · 11:12</span></div>
    <Tabs value={filter} onValueChange={setFilter} className="gap-5">
      <div className="max-w-full overflow-x-auto pb-1"><TabsList variant="line" aria-label="Filter activity"><TabsTrigger value="all">All <span className="work-filter-count">{executions.length}</span></TabsTrigger><TabsTrigger value="attention">Needs attention <span className="work-filter-count">{executions.filter(run => run.status !== "completed").length}</span></TabsTrigger><TabsTrigger value="completed">Completed</TabsTrigger></TabsList></div>
      <TabsContent value={filter}><div>{[TODAY, TODAY - 86400000].map(day => {
        const runs = visible.filter(run => run.started >= day && run.started < day + 86400000);
        return runs.length ? <section key={day} className="work-day-group" aria-label={date(day)}><h3 className="work-day-label">{day === TODAY ? "Today" : "Yesterday"}<span>{date(day)}</span></h3>{runs.map(run => <ExecutionRow key={run.id} run={run} open={element => openRun(run.id, element)} />)}</section> : null;
      })}</div></TabsContent>
    </Tabs>
  </div>;
}
function ExecutionRow({ run, open }: { run: Execution; open: (element: HTMLElement) => void }) {
  const Icon = statusIcons[run.status];
  return <article className="work-execution" id={`preview-${run.id}`}>
    <button data-run-id={run.id} className="work-execution-summary" onClick={event => open(event.currentTarget)} aria-label={`Inspect ${run.title}, ${statusLabel[run.status]}, ${time(run.started)}`}>
      <time className="work-run-time" dateTime={new Date(run.started).toISOString()}>{time(run.started)}</time>
      <span className={cn("work-run-status", run.status)}><Icon size={16} /></span>
      <span className="work-run-text"><strong>{run.title}</strong><span>{run.source}</span><span className="work-run-excerpt">{run.result}</span></span>
      <span className="work-run-outcome"><span className={cn(run.status === "interrupted" && "work-failure")}>{statusLabel[run.status]}</span><small>{duration(totalSeconds(run))}</small></span>
      <CaretRight size={14} />
    </button>
  </article>;
}
