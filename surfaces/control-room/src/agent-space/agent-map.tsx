import { useCallback, useEffect, useRef, useState } from "react";
import { Background, ReactFlow, ReactFlowProvider, ViewportPortal, useReactFlow, useStore, type Node } from "@xyflow/react";
import { ArrowsOutSimple, Cube, MagnifyingGlass, Minus, Pause, Play, Plus, X } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { NativeSelect, NativeSelectOption } from "@/components/ui/native-select";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { capabilityPlugins } from "../agent-work-preview/configuration-model";
import { useSavedPreviewConfiguration } from "../agent-work-preview/configuration-state";
import { usePreviewWork } from "../agent-work-preview/work-state";
import type { HostSnapshot } from "../host-contract";
import { agentHref, displayName, isEarlier } from "../host-presentation";
import { directorySummary } from "../host-agent-directory-model";
import { AgentSelection } from "./selection";
import { createScene, pluginMembers, previewManager, regionPath, type AgentScene } from "./scene";
import { isDistant, spaceNodeTypes, type PortraitNode, type RegionNode } from "./map-nodes";
import "@xyflow/react/dist/style.css";
import "../styles/agent-space.css";

const initialFit = { padding: .12, maxZoom: 1.15 };

export function AgentSpacePreview({ host, missingAgent }: { host: HostSnapshot; missingAgent: boolean }) {
  const savedConfiguration = useSavedPreviewConfiguration();
  const { exampleFor } = usePreviewWork();
  const [expanded, setExpanded] = useState(false);
  const [compact, setCompact] = useState(false);
  useEffect(() => {
    const media = window.matchMedia("(max-width: 600px)");
    const update = () => setCompact(media.matches);
    update(); media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  const agents = host.agents.filter(agent => !isEarlier(agent));
  const source = agents.map(agent => {
    const configuration = savedConfiguration(agent.id, displayName(agent.name));
    return { id: agent.id, name: configuration.name, originalName: agent.name, capabilityIds: configuration.capabilities,
      managerId: previewManager(agent, agents), summary: directorySummary(host, agent, exampleFor(agent.id)) };
  });
  const scene = createScene(source, expanded, compact ? 2 : 4);
  const earlier = host.agents.filter(isEarlier);
  return <main id="host-main" className="agent-space-page">
    <div className="space-heading"><div><h1>Agents <span>{scene.agents.length}</span></h1><p>Your agents, how they’re organized, and what they share.</p></div>
      <NativeSelect aria-label="Example scene" value={expanded ? "50" : "host"} onChange={event => setExpanded(event.target.value === "50")}>
        <NativeSelectOption value="host">Your agents · preview</NativeSelectOption><NativeSelectOption value="50">50-agent example</NativeSelectOption>
      </NativeSelect>
    </div>
    {missingAgent && <Alert><AlertDescription>That agent is not in this Host snapshot. Choose an available agent below.</AlertDescription></Alert>}
    <ReactFlowProvider key={`${expanded}-${compact}`}><AgentMap scene={scene} /></ReactFlowProvider>
    {earlier.length > 0 && <details className="space-earlier"><summary>Earlier identities <span>{earlier.length}</span></summary><ul>{earlier.map(agent => <li key={agent.id}><a href={agentHref(agent.id)}>{agent.name}</a></li>)}</ul></details>}
  </main>;
}

function AgentMap({ scene }: { scene: AgentScene }) {
  const flow = useReactFlow();
  const distant = useStore(isDistant);
  const [selected, setSelected] = useState<string | undefined>(scene.agents.find(agent => agent.managerId)?.id ?? scene.agents[0]?.id);
  const [pluginId, setPluginId] = useState("");
  const [query, setQuery] = useState("");
  const [paused, setPaused] = useState(false);
  const [motionAllowed, setMotionAllowed] = useState(false);
  const map = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!map.current) return;
    let width = map.current.clientWidth;
    const observer = new ResizeObserver(entries => {
      const next = entries[0]?.contentRect.width;
      if (next === undefined || Math.abs(next - width) < 1) return;
      width = next;
      void flow.fitView({ ...initialFit, duration: 0 });
    });
    observer.observe(map.current);
    return () => observer.disconnect();
  }, [flow]);
  useEffect(() => {
    const media = window.matchMedia("(prefers-reduced-motion: reduce)");
    let inView = true;
    const update = () => setMotionAllowed(!media.matches && document.visibilityState === "visible" && inView);
    const observer = new IntersectionObserver(entries => { inView = entries[0]?.isIntersecting ?? false; update(); });
    if (map.current) observer.observe(map.current);
    media.addEventListener("change", update);
    document.addEventListener("visibilitychange", update);
    update();
    return () => { observer.disconnect(); media.removeEventListener("change", update); document.removeEventListener("visibilitychange", update); };
  }, []);
  const duration = motionAllowed && !paused ? 350 : 0;
  const choose = useCallback((id: string) => { setSelected(id); setPluginId(""); }, []);
  const focusRegion = useCallback((id: string) => {
    const region = scene.regions.find(item => item.id === id);
    if (!region) return;
    choose(id);
    void flow.fitBounds(region.bounds, { padding: .16, duration });
  }, [flow, duration, choose, scene]);
  const focusAgent = (id: string) => {
    const agent = scene.agents.find(item => item.id === id);
    if (!agent) return;
    choose(id); setQuery("");
    void flow.setCenter(agent.position.x, agent.position.y + 20, { zoom: 1, duration });
  };
  const members = pluginMembers(scene.agents, pluginId);
  const memberIds = new Set(members.map(agent => agent.id));
  const term = query.trim().toLocaleLowerCase();
  const matches = scene.agents.filter(agent => `${agent.name} ${agent.id}`.toLocaleLowerCase().includes(term));
  const matchIds = new Set(matches.map(agent => agent.id));
  const nodes: Node[] = [
    ...scene.regions.map((region): RegionNode => ({ id: `region-${region.id}`, type: "region", position: { x: region.bounds.x, y: region.bounds.y },
      width: region.bounds.width, height: region.bounds.height, data: { region, focus: focusRegion }, selectable: false, draggable: false, zIndex: distant ? 3 : 0 })),
    ...scene.agents.map((agent): PortraitNode => {
      const parent = scene.agents.find(item => item.id === agent.managerId);
      const children = scene.agents.filter(item => item.managerId === agent.id).length;
      return { id: agent.id, type: "portrait", position: { x: agent.position.x - 90, y: agent.position.y - 65 }, width: 180, height: 180, zIndex: 2,
        data: { agent, active: selected === agent.id && !pluginId, dimmed: !!pluginId && !memberIds.has(agent.id) || !!term && !matchIds.has(agent.id), crowded: scene.agents.length > 6, choose,
          relationship: parent ? `Managed by ${parent.name}` : children ? `Manages ${children} ${children === 1 ? "agent" : "agents"}` : "Independent agent" } };
    }),
  ];
  const fit = () => void flow.fitView({ padding: .12, duration, maxZoom: 1.15 });
  const selectPlugin = (id: string) => { setQuery(""); setPluginId(current => current === id ? "" : id); };
  return <>
    <div className="space-toolbar">
      <div className="space-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon>
        <InputGroupInput type="search" aria-label="Find an agent" placeholder="Find an agent…" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => {
          if (event.key === "Escape") setQuery("");
          if (event.key === "Enter" && matches.length === 1) focusAgent(matches[0]!.id);
        }} />
        {query && <InputGroupAddon align="inline-end"><InputGroupButton aria-label="Clear search" size="icon-xs" onClick={() => setQuery("")}><X /></InputGroupButton></InputGroupAddon>}
      </InputGroup>{term && <div className="space-search-results"><p role="status">{matches.length ? `${matches.length} matching agents` : "No matching agents"}</p>{matches.map(agent => <button key={agent.id} onClick={() => focusAgent(agent.id)}>{agent.name}<span>Go to agent</span></button>)}</div>}</div>
      <div className="space-view-controls"><Button variant="outline" onClick={fit}><ArrowsOutSimple />Fit view</Button><Button variant="ghost" size="icon" aria-label={paused ? "Resume map motion" : "Pause map motion"} aria-pressed={paused} onClick={() => setPaused(value => !value)}>{paused ? <Play /> : <Pause />}</Button></div>
    </div>
    <div ref={map} className="space-map" data-motion={motionAllowed && !paused} aria-label="Agent relationship map" onFocusCapture={event => {
      const target = event.target;
      if (!(target instanceof HTMLButtonElement) || !map.current || !(target.dataset.agentId || target.dataset.regionId)) return;
      const item = target.getBoundingClientRect(), frame = map.current.getBoundingClientRect();
      if (item.left >= frame.left && item.right <= frame.right && item.top >= frame.top && item.bottom <= frame.bottom) return;
      const agent = scene.agents.find(candidate => candidate.id === (target.dataset.agentId ?? target.dataset.regionId));
      if (agent) void flow.setCenter(agent.position.x, agent.position.y, { zoom: flow.getZoom(), duration: 0 });
    }}>
      {scene.agents.length ? <ReactFlow nodes={nodes} nodeTypes={spaceNodeTypes} fitView fitViewOptions={initialFit} colorMode="dark"
        minZoom={.12} maxZoom={1.7} nodesDraggable={false} nodesConnectable={false} nodesFocusable={false} edgesFocusable={false} elementsSelectable={false}
        deleteKeyCode={null} selectionKeyCode={null} zoomOnDoubleClick={false} zoomOnScroll={false} zoomOnPinch preventScrolling={false}
        attributionPosition="bottom-left" aria-label="Explore agent spaces" onPaneClick={() => { setQuery(""); setPluginId(""); }}>
        <Background gap={28} size={.7} color="#ffffff19" />
        {pluginId && members.length > 0 && <ViewportPortal><svg className="space-shared-field" aria-hidden="true"><path d={regionPath(members.map(agent => agent.position), 128)} /></svg>
          <div className="space-capability-label" style={{ left: members.reduce((sum, agent) => sum + agent.position.x, 0) / members.length, top: Math.min(...members.map(agent => agent.position.y)) - 90 }}><Cube size={18} /><span>{capabilityPlugins.find(plugin => plugin.id === pluginId)!.name}<small>{members.length} agents · shared access</small></span></div>
        </ViewportPortal>}
        <ZoomControls />
      </ReactFlow> : <div className="space-map-empty">No agents yet. Agents will appear here when registered on this Host.</div>}
    </div>
    <div className="space-plugins" aria-label="Shared capabilities"><span><Cube size={16} />Shared capabilities</span><div>{capabilityPlugins.map(plugin => {
      const count = pluginMembers(scene.agents, plugin.id).length;
      return <button key={plugin.id} className="space-plugin" data-active={pluginId === plugin.id} aria-pressed={pluginId === plugin.id} aria-label={`${plugin.name}, ${count} agents`} onClick={() => selectPlugin(plugin.id)}><i aria-hidden="true" />{plugin.name}<span>{count}</span></button>;
    })}</div></div>
    <div className="space-caption"><span>{pluginId ? "Shared access crosses agent spaces. It doesn’t imply shared context." : "Regions show management. Select a plugin to reveal shared access."}</span><span>Drag to pan · Pinch to zoom</span></div>
    <AgentSelection agent={scene.agents.find(agent => agent.id === selected)} scene={scene} pluginId={pluginId} onSelect={focusAgent} />
  </>;
}

function ZoomControls() {
  const flow = useReactFlow();
  const zoom = useStore(state => state.transform[2]);
  return <div className="space-zoom-controls"><Button variant="ghost" size="icon" aria-label="Zoom out" disabled={zoom <= .121} onClick={() => void flow.zoomOut()}><Minus /></Button><output aria-label="Map zoom">{Math.round(zoom * 100)}%</output><Button variant="ghost" size="icon" aria-label="Zoom in" disabled={zoom >= 1.69} onClick={() => void flow.zoomIn()}><Plus /></Button></div>;
}
