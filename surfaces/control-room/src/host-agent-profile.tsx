import { useEffect, useRef, useState } from "react";
import { ArrowLeft, CirclesThree, SlidersHorizontal } from "@phosphor-icons/react";
import { Avatar, AvatarFallback, AvatarImage } from "@/components/ui/avatar";
import { Badge } from "@/components/ui/badge";
import { Button } from "@/components/ui/button";
import { Tabs, TabsContent, TabsList, TabsTrigger } from "@/components/ui/tabs";
import type { Agent, HostSnapshot } from "./host-contract";
import { agentPage, type AgentPage, type AgentSection } from "./host-navigation";
import type { Controls } from "./host-controls";
import { agentHref, displayName } from "./host-presentation";
import { agentOverview } from "./host-agent-overview";
import { portraitForAgent } from "./host-identity";
import { ProfileOverview } from "./host-agent-profile-overview";
import { ProfileConfigure } from "./host-agent-profile-configure";
import { ProfileAutomations, ProfileActivity } from "./host-agent-profile-details";
import "./styles/host-agent-profile.css";
import { AgentWorkPreview } from "./agent-work-preview/preview";
import { usePreviewConfiguration } from "./agent-work-preview/configuration-state";
import { workDesignPreview } from "./agent-work-preview/mode";

const pages: { value: AgentPage; label: string }[] = [
  { value: "overview", label: "Overview" }, { value: "configure", label: "Configure" },
  { value: "automations", label: "Automations" }, { value: "activity", label: "Activity" },
];

export function HostAgentProfile({ host, agent, section, controls, execution }: { host: HostSnapshot; agent: Agent; section: AgentSection; controls: Controls; execution: string | undefined }) {
  const page = agentPage(section);
  const designPreview = workDesignPreview(controls.preview);
  const configuration = usePreviewConfiguration(agent.id, displayName(agent.name));
  const agentName = designPreview ? configuration.saved.name : displayName(agent.name);
  const inspecting = designPreview && execution !== undefined;
  const [visited, setVisited] = useState<AgentPage[]>([page]);
  const root = useRef<HTMLElement>(null);
  const previous = useRef(section);
  const data = agentOverview(host, agent);
  function navigate(next: AgentSection) { window.location.hash = agentHref(agent.id, next); }
  useEffect(() => {
    setVisited(values => values.includes(page) ? values : [...values, page]);
    // Existing deep links land at their matching in-page section.
    const target = section === "connections" ? "profile-connections" : section === "policy" ? "profile-policy" : null;
    const heading = target ? root.current?.querySelector<HTMLElement>(`#${target}`) : null;
    const frame = heading ? requestAnimationFrame(() => { heading.focus({ preventScroll: true }); heading.scrollIntoView({ block: "start" }); }) : null;
    if (!heading && previous.current !== section) root.current?.querySelector('[role="tablist"]')?.scrollIntoView({ block: "nearest" });
    previous.current = section;
    return () => { if (frame !== null) cancelAnimationFrame(frame); };
  }, [page, section]);
  return <main ref={root} id="host-main" className="renoa-agent-profile mx-auto flex w-full max-w-6xl flex-col gap-7 px-4 py-6 md:px-8 md:py-8">
    <div hidden={inspecting} className="profile-identity-wrap flex flex-col gap-7"><Button asChild variant="ghost" size="sm" className="w-fit"><a href="#agents"><ArrowLeft data-icon="inline-start" />All agents</a></Button>
    <section aria-label="Agent identity" className="grid grid-cols-[auto_minmax(0,1fr)] items-center gap-5 sm:grid-cols-[auto_minmax(0,1fr)_auto]">
      <div className="renoa-agent-portrait relative shrink-0">
        <Avatar className="size-16"><AvatarImage src={portraitForAgent(agent.id, agent.name)} alt="" /><AvatarFallback><CirclesThree /></AvatarFallback></Avatar>
        <img className="absolute -right-1 -bottom-1 size-6 rounded-full border-2 border-background" src="/assets/identities/renoa-host-gold.webp" alt="" aria-hidden="true" />
      </div>
      <div className="flex min-w-0 flex-col gap-2">
        <span className="text-xs text-muted-foreground">Renoa agent · Cloud Host</span>
        <h1 className="break-words text-3xl font-semibold tracking-tight">{agentName}</h1>
        <div className="flex flex-wrap items-center gap-2"><Badge variant="outline">Host-owned</Badge>
          {data.activity.tone === "attention" && <Badge variant="destructive">Needs attention</Badge>}
          {data.activity.tone === "pending" && <Badge variant="secondary">Unfinished work</Badge>}
        </div>
      </div>
      <Button variant="outline" className="col-span-2 w-fit sm:col-span-1" onClick={() => navigate("configure")}><SlidersHorizontal data-icon="inline-start" />Customize</Button>
    </section></div>
    <Tabs value={page} activationMode="manual" onValueChange={value => navigate(value as AgentPage)} className="gap-7">
      <div hidden={inspecting} className="overflow-x-auto border-b pb-1"><TabsList variant="line" aria-label="Agent workspace" className="w-full justify-start sm:w-fit">
        {pages.map(item => <TabsTrigger key={item.value} value={item.value} className="sm:px-4">{item.label}</TabsTrigger>)}
      </TabsList></div>
      {designPreview && <AgentWorkPreview page={page} navigate={navigate} execution={execution} agentId={agent.id} agentName={agentName} configuration={configuration} />}
      {pages.filter(() => !designPreview).map(({ value }) => <TabsContent key={value} value={value} forceMount hidden={inspecting || page !== value}>
        {(page === value || visited.includes(value)) && <>
          {value === "overview" && <ProfileOverview {...{ host, agent, navigate }} />}
          {value === "configure" && <ProfileConfigure {...{ host, agent }} />}
          {value === "automations" && <ProfileAutomations {...{ data, controls }} />}
          {value === "activity" && <ProfileActivity data={data} preview={controls.preview} active={page === value} />}
        </>}
      </TabsContent>)}
    </Tabs>
  </main>;
}
