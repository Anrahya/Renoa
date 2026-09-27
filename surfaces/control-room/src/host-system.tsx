import { useState } from "react";
import { ArrowsLeftRight, ArrowRight, ArrowClockwise, Database } from "@phosphor-icons/react";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { HostSnapshot } from "./host-contract";
import { PageHeading, NoResults, agentCount } from "./host-desk";
import { systemComponents } from "./host-system-components";
import { attentionReviews, sessionNeedsAttention, timestamp } from "./host-presentation";
import { ReviewRow, SessionRow } from "./host-records";
import "./styles/host-system-preview.css";

export function SystemView({ host, live, receivedAt, refresh, preview }: {
  host: HostSnapshot; live: boolean; receivedAt: number | null; refresh: () => void; preview: boolean;
}) {
  const [tab, setTab] = useState("health");
  const [selected, select] = useState("coordinator");
  const reviews = attentionReviews(host.reviews);
  const sessions = host.sessions.filter(sessionNeedsAttention);
  const current = systemComponents.find(component => component.id === selected)!;
  return <main id="host-main" className="host-desk">
    <PageHeading title="System" description="The infrastructure behind your agents." />
    <div className="system-summary"><span className="system-summary-signal" data-tone={live ? "ready" : "offline"} /><div><h2>{live ? "Host observation connected" : "Showing saved records"}</h2><p>Service health and resource usage are not reported by this connection.</p></div><span className="desk-note">{receivedAt !== null ? `Received ${timestamp(receivedAt)}` : "No receipt time available"}</span></div>
    <Tabs value={tab} onValueChange={setTab} className="gap-7"><TabsList variant="line" aria-label="System sections"><TabsTrigger value="health">Health</TabsTrigger><TabsTrigger value="diagnostics">Diagnostics <Badge variant="secondary">{reviews.length + sessions.length}</Badge></TabsTrigger><TabsTrigger value="recovery">Recovery</TabsTrigger></TabsList>
      <TabsContent value="health">
        <section className="system-architecture" aria-label="System components"><div className="desk-section-heading"><h2>One system, separate responsibilities</h2><span className="desk-note">Select a component to inspect it</span></div>
          <div className="system-topology">{systemComponents.map((component, index) => <div className="system-topology-part" key={component.id}>
            {index > 0 && <div className="system-link" aria-label="RCP connection"><ArrowsLeftRight size={20} /><span>RCP</span></div>}
            <button className="system-component" aria-pressed={selected === component.id} onClick={() => select(component.id)}><component.Icon size={25} /><strong>{component.title}</strong><small>{component.subtitle}</small><span className="desk-status">{component.id === "surface" ? "Open" : "Health unknown"}</span></button>
          </div>)}</div>
          <div className="system-storage"><Database size={17} /><strong>Durable state</strong><span>Task journal at the coordinator</span><span>Execution records at the node</span><Badge variant="outline">Retained records</Badge></div>
          <div className="system-component-detail" aria-live="polite"><h3>{current.title}</h3><p>{current.description}</p><dl className="desk-facts">{current.facts.map(([label,value]) => <div key={label}><dt>{label}</dt><dd>{value}</dd></div>)}</dl></div>
        </section>
        <section className="system-resources"><div className="desk-section-heading"><h2>Cloud Host resources</h2><span className="desk-note">Metrics not available</span></div><div className="system-resource-grid">{["CPU", "Memory", "Storage"].map(name => <div className="system-resource" key={name}><h3>{name}</h3><p><strong>—</strong><span>No measurement reported</span></p></div>)}</div></section>
        <div className="system-destinations"><a href="#agents"><strong>{agentCount(host.agents.length)}</strong><span>Explore the agent map</span><ArrowRight size={16} /></a><a href="#library"><strong>{host.plugins.length} installed plugins</strong><span>Shared capabilities</span><ArrowRight size={16} /></a><a href="#library/accounts"><strong>{host.connections.length} saved connections</strong><span>Stored catalogs & selections</span><ArrowRight size={16} /></a></div>
      </TabsContent>
      <TabsContent value="diagnostics"><div className="desk-section-heading"><div><h2>Recorded interruptions</h2><p>Retained failures and uncertain outcomes, separate from service health.</p></div><Button asChild variant="outline" size="sm"><a href="#work">All work<ArrowRight data-icon="inline-end" /></a></Button></div>
        {reviews.map(review => <ReviewRow key={review.request_id} {...{ host, review, preview }} />)}{sessions.map(session => <SessionRow key={session.id} {...{ host, session }} />)}
        {!reviews.length && !sessions.length && <NoResults title="No interruptions recorded" />}
        <details className="desk-disclosure mt-7"><summary><span className="desk-disclosure-title"><strong>Host identity</strong><small>For diagnostics and support</small></span></summary><div className="desk-disclosure-body"><code className="break-all text-xs">{host.host_id}</code></div></details>
      </TabsContent>
      <TabsContent value="recovery"><section className="system-recovery"><div><h2>Refresh the observation</h2><p>Request the latest retained records from your Host.</p></div><div className="system-recovery-controls"><Button variant="outline" className="w-fit" onClick={refresh}><ArrowClockwise data-icon="inline-start" />Refresh Host records</Button><p className="desk-note">Process restart controls are not available through this panel.</p></div></section></TabsContent>
    </Tabs>
  </main>;
}
