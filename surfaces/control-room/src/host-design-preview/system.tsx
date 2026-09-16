import { useEffect, useRef, useState } from "react";
import { ArrowsLeftRight, ArrowRight, ArrowClockwise, Browser, Database, Cpu, HardDrives, WarningCircle } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Field, FieldGroup, FieldLabel, FieldDescription } from "@/components/ui/field";
import { Alert, AlertDescription, AlertTitle } from "@/components/ui/alert";
import type { HostSnapshot } from "../host-contract";
import { capabilityPlugins } from "../agent-work-preview/configuration-model";
import { usePreviewConnections } from "../agent-work-preview/connection-state";
import { time, totalSeconds } from "../agent-work-preview/data";
import { issueFor, traceEvents } from "../agent-work-preview/trace-model";
import { PageHeading, useDesignAgents, NoResults } from "./shared";
import "../styles/host-system-preview.css";

type Condition = "ready" | "kernel" | "offline";
const components = [
  { id: "surface", title: "Control panel", subtitle: "Observe & control", Icon: Browser, description: "This browser presents durable work and sends authorized commands. Closing it does not stop an agent.", facts: [["Connection", "RCP over WebSocket"], ["Identity", "Remembered browser"], ["On reconnect", "Replay from saved position"]] },
  { id: "host", title: "Host coordinator", subtitle: "Admit, route & retain", Icon: HardDrives, description: "The Host admits commands and retains the task journal. Acknowledged work survives a dropped connection; admission is separate from execution.", facts: [["Owns", "Task identity & admission"], ["Journal", "Durable, ordered records"], ["Retries", "Stable command identities"]] },
  { id: "kernel", title: "Execution node", subtitle: "Renoa kernel", Icon: Cpu, description: "The node owns the workspace, tools and model access. The kernel is its replaceable execution harness, separate from the Host coordinator.", facts: [["Harness", "Renoa kernel"], ["Environment", "Cloud workspace"], ["Owns", "Model and tool execution"]] },
];
const resources = [
  { name: "CPU", value: "22%", detail: "of 2 virtual CPUs", unit: "%", values: [12,14,13,18,26,21,35,32,28,24,20,22], max: 100 },
  { name: "Memory", value: "1.2 GB", detail: "of 4 GB", unit: "GB", values: [.8,.8,.9,1,1.1,1.3,1.2,1.2,1.3,1.2,1.2,1.2], max: 4 },
  { name: "Storage", value: "7.8 GB", detail: "of 40 GB", unit: "GB", values: [7.4,7.4,7.4,7.5,7.5,7.6,7.6,7.6,7.7,7.7,7.8,7.8], max: 40 },
];
export function SystemPreview({ host }: { host: HostSnapshot }) {
  const agents = useDesignAgents(host);
  const { connected } = usePreviewConnections();
  const [tab, setTab] = useState("health");
  const [selected, select] = useState("host");
  const [condition, setCondition] = useState<Condition>("ready");
  const [target, setTarget] = useState("kernel");
  const [confirming, confirm] = useState(false);
  const [notice, setNotice] = useState("");
  const [restarts, setRestarts] = useState(0);
  const recoveryAction = useRef<HTMLButtonElement>(null);
  useEffect(() => { recoveryAction.current?.focus({ preventScroll: true }); }, [confirming]);
  const offline = condition === "offline";
  const diagnosticRows = agents.flatMap(agent => agent.executions.filter(run => run.status === "interrupted").map(run => {
    const issue = [...traceEvents(run)].reverse().map(issueFor).find(issue => issue && !issue.recovered);
    return { agent, run, issue, at: run.started + totalSeconds(run) * 1000 };
  })).sort((a,b) => b.at - a.at);
  function scenario(next: Condition) { setCondition(next); confirm(false); setNotice(""); }
  function restart() { setRestarts(value => value + 1); if (target === "kernel") setCondition("ready"); confirm(false); setNotice(`${target === "kernel" ? "Kernel" : "Host coordinator"} restarted in this preview. Saved records are unchanged; interrupted runs still need review.`); }
  const current = components.find(component => component.id === selected)!;
  return <main id="host-main" className="host-desk">
    <PageHeading title="System" description="The infrastructure behind your agents."><NativeSelect aria-label="Example system condition" value={condition} onChange={event => scenario(event.target.value as Condition)}><NativeSelectOption value="ready">Example: healthy core</NativeSelectOption><NativeSelectOption value="kernel">Example: kernel stopped</NativeSelectOption><NativeSelectOption value="offline">Example: Host unreachable</NativeSelectOption></NativeSelect></PageHeading>
    <div className="system-summary"><span className="system-summary-signal" data-tone={condition} /><div><h2>{offline ? "Host unreachable" : condition === "kernel" ? "Execution needs attention" : "Core services available"}</h2><p>{offline ? "Showing the last observation. Current service health is unknown." : condition === "kernel" ? "The Host is reachable. The kernel isn’t accepting work." : "The Host, execution node and durable storage are available."}</p></div><span className="desk-note">Example observation · 11:30</span></div>
    <Tabs value={tab} onValueChange={value => { setTab(value); confirm(false); }} className="gap-7"><TabsList variant="line" aria-label="System sections"><TabsTrigger value="health">Health</TabsTrigger><TabsTrigger value="diagnostics">Diagnostics <Badge variant="secondary">{diagnosticRows.length}</Badge></TabsTrigger><TabsTrigger value="recovery">Recovery</TabsTrigger></TabsList>
      <TabsContent value="health">
        <section className="system-architecture" aria-label="System components"><div className="desk-section-heading"><h2>One system, separate responsibilities</h2><span className="desk-note">Select a component to inspect it</span></div>
          <div className="system-topology">{components.map((component, index) => <div className="system-topology-part" key={component.id}>
            {index > 0 && <div className="system-link" aria-label="RCP connection"><ArrowsLeftRight size={20} /><span>RCP</span></div>}
            <button className="system-component" aria-pressed={selected === component.id} onClick={() => select(component.id)}><component.Icon size={25} /><strong>{component.title}</strong><small>{component.subtitle}</small><span className="desk-status" data-tone={offline && component.id !== "surface" ? "warning" : component.id === "kernel" && condition === "kernel" ? "error" : "good"}>{component.id === "surface" ? "Open" : offline ? "Unknown" : component.id === "kernel" && condition === "kernel" ? "Stopped" : "Available"}</span></button>
          </div>)}</div>
          <div className="system-storage"><Database size={17} /><strong>Durable state</strong><span>Task journal at the Host</span><span>Execution records at the node</span><Badge variant="outline">{offline ? "Last observed: saved" : "Persisted"}</Badge></div>
          <div className="system-component-detail" aria-live="polite"><h3>{current.title}</h3><p>{current.description}</p><dl className="desk-facts">{current.facts.map(([label,value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl></div>
        </section>
        <section className="system-resources"><div className="desk-section-heading"><h2>Cloud Host resources</h2><span className="desk-note">{offline ? "Last saved samples" : "Example samples"} · 11:00–11:30</span></div><div className="system-resource-grid">{resources.map(resource => <Resource key={resource.name} resource={resource} />)}</div></section>
        <div className="system-destinations"><a href="#agents"><strong>{agents.length} agents</strong><span>Explore the agent map</span><ArrowRight size={16} /></a><a href="#library"><strong>{capabilityPlugins.length} plugins</strong><span>Shared capabilities</span><ArrowRight size={16} /></a><a href="#library/accounts"><strong>{Object.values(connected).filter(Boolean).length} accounts connected</strong><span>Access & credentials</span><ArrowRight size={16} /></a></div>
      </TabsContent>
      <TabsContent value="diagnostics"><div className="desk-section-heading"><div><h2>Recorded interruptions</h2><p>Run failures are separate from the current health of a service.</p></div><Button asChild variant="outline" size="sm"><a href="#work">All work<ArrowRight data-icon="inline-end" /></a></Button></div>
        {diagnosticRows.map(({ agent, run, issue, at }) => <a className="system-diagnostic" key={`${agent.id}-${run.id}`} href={`#work/${encodeURIComponent(agent.id)}/${encodeURIComponent(run.id)}`}><WarningCircle size={19} /><time>{time(at)}</time><span><strong>{issue?.origin ?? "Execution"} · {issue?.code ?? "interrupted"}</strong><p>{issue?.message ?? run.result}</p><small>{agent.name} / {run.title}</small></span><ArrowRight size={17} /></a>)}
        {!diagnosticRows.length && <NoResults title="No interruptions recorded" />}
        <details className="desk-disclosure mt-7"><summary><span className="desk-disclosure-title"><strong>Host identity</strong><small>For diagnostics and support</small></span></summary><div className="desk-disclosure-body"><code className="break-all text-xs">{host.host_id}</code><p className="desk-note">Design preview. Diagnostic entries link to the same example executions shown in Work.</p></div></details>
      </TabsContent>
      <TabsContent value="recovery"><section className="system-recovery"><div><h2>Restart a component</h2><p>Recover an execution process without removing its durable work.</p><p className="desk-note">A restart doesn’t prove a failed run completed or retry its external side effects. Inspect interrupted work after recovery.</p></div>
        <div className="system-recovery-controls"><FieldGroup><Field><FieldLabel htmlFor="recovery-target">Component</FieldLabel><NativeSelect id="recovery-target" value={target} onChange={event => { setTarget(event.target.value); confirm(false); setNotice(""); }}><NativeSelectOption value="kernel">Renoa kernel</NativeSelectOption><NativeSelectOption value="host">Host coordinator</NativeSelectOption></NativeSelect><FieldDescription>{target === "kernel" ? "Restarts the execution harness. Active model and tool calls may be interrupted." : "Restarts task admission and routing. Browsers reconnect to the retained journal."}</FieldDescription></Field></FieldGroup>
          {offline ? <Alert><AlertTitle>Recovery is unavailable from this connection</AlertTitle><AlertDescription>The Host must be reachable to accept a restart. If the machine is down, use the VPS provider’s console.</AlertDescription></Alert> : confirming ? <Alert><AlertTitle>Simulate a {target === "kernel" ? "kernel" : "Host"} restart?</AlertTitle><AlertDescription>This changes example health only. No process on the VPS will be restarted.</AlertDescription><div className="mt-4 flex gap-2"><Button ref={recoveryAction} onClick={restart}>Simulate restart</Button><Button variant="ghost" onClick={() => confirm(false)}>Cancel</Button></div></Alert> : <Button ref={recoveryAction} variant="outline" className="w-fit" onClick={() => confirm(true)}><ArrowClockwise data-icon="inline-start" />Restart {target === "kernel" ? "kernel" : "Host"}</Button>}
          {notice && <Alert><AlertDescription role="status">{notice}</AlertDescription></Alert>}
        </div></section>
        <section className="system-recovery-record"><div className="desk-section-heading"><h3>Recovery record</h3><Badge variant="outline">This preview</Badge></div><p className="desk-note">{restarts ? `${restarts} restart ${restarts === 1 ? "simulation" : "simulations"} completed. No live services changed.` : "No recovery actions in this preview."}</p></section>
      </TabsContent>
    </Tabs>
  </main>;
}
function Resource({ resource }: { resource: typeof resources[number] }) {
  const { values, max } = resource;
  const points = values.map((value, index) => `${index / (values.length - 1) * 240},${65 - value / max * 55}`).join(" ");
  return <div className="system-resource"><h3>{resource.name}</h3><p><strong>{resource.value}</strong><span>{resource.detail}</span></p><svg viewBox="0 0 240 72" role="img" aria-label={`${resource.name} over the example half-hour. Current ${resource.value}.`}><line x1="0" y1="65" x2="240" y2="65" /><polyline points={points} />{values.map((value,index) => <circle key={index} cx={index / (values.length - 1) * 240} cy={65 - value / max * 55} r="3"><title>{value} {resource.unit}</title></circle>)}</svg></div>;
}
