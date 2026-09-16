import { useEffect, useRef, useState } from "react";
import { TabsContent } from "@/components/ui/tabs";
import { Button } from "@/components/ui/button";
import type { AgentPage, AgentSection } from "../host-navigation";
import { automations, executions } from "./data";
import { AutomationTimeline } from "./automations";
import { ActivityExplorer } from "./activity";
import { WorkOverview } from "./overview";
import { ConfigurePreview } from "./configure";
import type { Configuration } from "./configuration-model";
import { RunPage } from "./run-page";
import "../styles/agent-work-preview.css";
import "../styles/agent-run-preview.css";
type Props = { configuration: Configuration; saveConfiguration: (value: Configuration) => void; page: AgentPage; navigate: (section: AgentSection) => void; execution: string | undefined; agentId: string; agentName: string };
export function AgentWorkPreview({ page, navigate, execution, agentId, agentName, configuration, saveConfiguration }: Props) {
  const [enabled, setEnabled] = useState<Record<string, boolean>>(() => Object.fromEntries(automations.map(item => [item.id, item.enabled])));
  const [filter, setFilter] = useState("all");
  const [selectedAutomation, selectAutomation] = useState<string | null>(null);
  const origin = useRef<{ page: AgentPage; scroll: number; runId: string | null; element: HTMLElement | null }>({ page: "activity", scroll: 0, runId: null, element: null });
  const previousRun = useRef(execution);
  const pendingFocus = useRef(false);
  useEffect(() => {
    const returning = previousRun.current !== undefined && execution === undefined;
    previousRun.current = execution;
    if (execution) return;
    const frame = requestAnimationFrame(() => {
      if (pendingFocus.current && page === "automations") {
        pendingFocus.current = false;
        const element = document.getElementById("automation-inspection");
        element?.scrollIntoView({ block: "nearest" });
        element?.querySelector<HTMLElement>("button")?.focus({ preventScroll: true });
      } else if (returning && page === origin.current.page) {
        window.scrollTo({ top: origin.current.scroll });
        const target = [...document.querySelectorAll<HTMLElement>("[data-run-id]")].find(element => element.dataset.runId === origin.current.runId && element.getClientRects().length > 0);
        target?.focus({ preventScroll: true });
      }
    });
    return () => cancelAnimationFrame(frame);
  }, [page, execution, selectedAutomation]);
  function openRun(id: string, element: HTMLElement) {
    origin.current = { page, scroll: window.scrollY, runId: id, element };
    window.location.hash = `#agent/${encodeURIComponent(agentId)}/activity/${encodeURIComponent(id)}`;
  }
  function openAutomation(id: string) { selectAutomation(id); pendingFocus.current = true; navigate("automations"); }
  const run = executions.find(item => item.id === execution);
  const back = () => origin.current.element ? window.history.back() : navigate(origin.current.page);
  return <>
    <TabsContent value="configure" forceMount hidden={!!execution || page !== "configure"}><div className="agent-work-preview"><ConfigurePreview saved={configuration} save={saveConfiguration} /></div></TabsContent>
    <TabsContent value="overview" forceMount hidden={!!execution || page !== "overview"}><div className="agent-work-preview"><WorkOverview {...{ enabled, openRun, openAutomation, navigate }} /></div></TabsContent>
    <TabsContent value="automations" forceMount hidden={!!execution || page !== "automations"}><div className="agent-work-preview"><AutomationTimeline enabled={enabled} setEnabled={(id, value) => setEnabled(current => ({ ...current, [id]: value }))} selected={selectedAutomation} select={selectAutomation} openRun={openRun} /></div></TabsContent>
    <TabsContent value="activity" forceMount hidden={!!execution || page !== "activity"}><div className="agent-work-preview"><ActivityExplorer {...{ filter, setFilter, openRun }} /></div></TabsContent>
    {execution && <div className="agent-work-preview">{run ? <RunPage key={run.id} {...{ run, agentName, back, openAutomation }} backLabel={origin.current.page === "overview" ? "Overview" : origin.current.page === "automations" ? "Automations" : "Activity"} /> : <div className="run-empty"><h1>Execution not found</h1><p>This execution is not available in the design preview.</p><Button variant="outline" onClick={back}>Back to Activity</Button></div>}</div>}
  </>;
}
