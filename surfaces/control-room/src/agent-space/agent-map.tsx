import { useCallback, useEffect, useMemo, useRef, useState, type Dispatch, type SetStateAction, type ReactNode } from "react";
import { Background, ReactFlow, ReactFlowProvider, ViewportPortal, useReactFlow, useStore, type Node } from "@xyflow/react";
import { ArrowRight, ArrowsOutSimple, Cube, MagnifyingGlass, Minus, Pause, Play, Plus, X } from "@phosphor-icons/react";
import { Button } from "@/components/ui/button";
import { InputGroup, InputGroupAddon, InputGroupButton, InputGroupInput } from "@/components/ui/input-group";
import { Alert, AlertDescription } from "@/components/ui/alert";
import { agentHref, timestamp } from "../host-presentation";
import { portraitForAgent } from "../host-identity";
import { agentCount } from "../host-desk";
import { agentStates, emptyCounts, stateLabel, type AgentState } from "../host-state";
import { createScene, pluginMembers, regionPath, type AgentScene, type DayAxis, type PlacedAgent, type SpaceAgent, type SpacePlugin } from "./scene";
import { DayTrack, isDistant, spaceNodeTypes, type PortraitNode, type RegionNode } from "./map-nodes";
import "@xyflow/react/dist/style.css";
import "../styles/agent-space.css";

const initialFit = { padding: .12, maxZoom: 1.15 };

// Search, selection, plugin highlight and pause sit above the keyed provider:
// crossing the 600px breakpoint remounts the map to refit the new column
// layout, and that remount must not discard what the user was looking at.
type MapUi = { selected: string | undefined; pluginId: string; query: string; paused: boolean };
const initialMapUi: MapUi = { selected: undefined, pluginId: "", query: "", paused: false };

export function AgentSpace({ source, plugins, missingAgent, children, selector }: {
  source: SpaceAgent[]; plugins: SpacePlugin[]; missingAgent: boolean; children?: ReactNode; selector?: ReactNode;
}) {
  const [compact, setCompact] = useState(false);
  const [mapUi, setMapUi] = useState<MapUi>(initialMapUi);
  useEffect(() => {
    const media = window.matchMedia("(max-width: 600px)");
    const update = () => setCompact(media.matches);
    update(); media.addEventListener("change", update);
    return () => media.removeEventListener("change", update);
  }, []);
  const scene = createScene(source, compact ? 2 : 4);
  const counts = useMemo(() => {
    const tally = emptyCounts();
    for (const agent of scene.agents) tally[agent.state] += 1;
    return tally;
  }, [scene]);
  const waiting = scene.agents.filter(agent => agent.state === "needs-you");
  const failed = scene.agents.filter(agent => agent.state === "failed");
  return <main id="host-main" className="agent-space-page">
    <div className="space-heading"><div><h1>Agents <span>{scene.agents.length}</span></h1>
      <p>{[
        waiting.length ? `${waiting.length} waiting on you` : "Nothing waiting on you",
        failed.length ? `${failed.length} failed` : null,
      ].filter(Boolean).join(" · ")}</p></div>
      {selector}
    </div>
    <div className="space-create">{children}</div>
    <TodayBand agents={scene.agents} axis={scene.axis} />
    {missingAgent && <Alert><AlertDescription>That agent is not in this Host snapshot. Choose an available agent below.</AlertDescription></Alert>}
    <AttentionRail agents={[...waiting, ...failed]} />
    <ReactFlowProvider key={String(compact)}><AgentMap scene={scene} plugins={plugins} ui={mapUi} setUi={setMapUi} /></ReactFlowProvider>
    <StateLegend counts={counts} />
  </main>;
}

// What the whole system did today, in one band. The rail answers "what needs
// me"; this answers "what has been happening", so the landing screen carries
// both questions without anyone opening the map or Work.
function TodayBand({ agents, axis }: { agents: PlacedAgent[]; axis: DayAxis }) {
  const day = agents.flatMap(agent => agent.summary?.day ?? []);
  const rows = agents.map(agent => ({ agent, day: agent.summary?.day ?? [] })).filter(entry => entry.day.length > 0);
  if (day.length === 0 || !axis) {
    return <div className="space-today space-today-empty" role="status">
      <span className="space-today-label">Today</span>
      <p>No work recorded today. Records appear here as the Host admits them.</p>
    </div>;
  }
  const clock = (at: number) => new Date(at).toLocaleTimeString(undefined, { hour: "2-digit", minute: "2-digit", hour12: false });
  return <section className="space-today" aria-label="Today's recorded work">
    <span className="space-today-head">
      <span className="space-today-label">Today</span>
      <b>{day.length}</b>
    </span>
    <div className="space-today-rows">
      {rows.map(({ agent, day: agentDay }) => <div className="space-today-row" key={agent.id}>
        <span className="space-today-name" title={agent.name}>{agent.name}</span>
        <DayTrack marks={agentDay} axis={axis} className="space-today-track" />
        <span className="space-today-count">{agentDay.length}</span>
      </div>)}
    </div>
    <div className="space-today-scale" aria-hidden="true">
      <span>{clock(axis.start)}</span>
      <span>{clock(axis.end)}</span>
    </div>
  </section>;
}

// The one thing the owner can act on, above everything else. Two lanes, never
// merged: an agent waiting on a decision is gold because it is a question, and
// an agent that broke is coral because it is a fault. Collapsing them is what
// made every attention record look like an error.
function AttentionRail({ agents }: { agents: PlacedAgent[] }) {
  const [open, setOpen] = useState(true);
  const waiting = agents.filter(agent => agent.state === "needs-you").length;
  const failed = agents.length - waiting;
  if (agents.length === 0) {
    return <div className="space-clear" role="status">
      <span className="space-clear-mark" aria-hidden="true" />
      <p><strong>Nothing needs you.</strong> There are no attention flags in the retained records.</p>
    </div>;
  }
  return <section className="space-waiting" aria-labelledby="waiting-heading" data-open={open}>
    <button className="space-waiting-toggle" aria-expanded={open} aria-controls="waiting-list" onClick={() => setOpen(value => !value)}>
      <h2 id="waiting-heading">Needs you <span>{waiting}</span></h2>
      {failed > 0 && <span className="space-waiting-failed" data-state="failed">Failed <span>{failed}</span></span>}
      <span className="space-waiting-hint">{open ? "Hide" : "Show"}</span>
    </button>
    {open && <ul id="waiting-list" className="space-waiting-list">
      {agents.map(agent => <li key={agent.id} data-state={agent.state}>
        <a href={agent.summary?.workHref ?? agentHref(agent.id, "activity")}>
          <img className="space-waiting-avatar" src={portraitForAgent(agent.id, agent.originalName)} alt="" aria-hidden="true" draggable={false} />
          <span className="space-waiting-body">
            <strong>{agent.summary?.title ?? (agent.state === "failed" ? "Work stopped before it finished" : "Open work needs a decision")}</strong>
            <small>{agent.summary?.detail ?? "Inspect the retained record and diagnostics."}</small>
          </span>
          <span className="space-waiting-agent">{agent.name}{agent.summary?.lastAt != null && <em>{timestamp(agent.summary.lastAt)}</em>}</span>
          <ArrowRight className="space-waiting-arrow" aria-hidden="true" />
        </a>
      </li>)}
    </ul>}
  </section>;
}

// One legend, one owner: the four states the map tints by, with the counts that
// produced them. A space on the map is only meaningful if the colours are named.
function StateLegend({ counts }: { counts: Record<AgentState, number> }) {
  return <div className="space-legend" aria-label="Agent states">
    <span className="space-legend-title">State</span>
    <ul>{agentStates.map(state => <li key={state} data-state={state}>
      <i aria-hidden="true" />{stateLabel[state]}<b>{counts[state]}</b>
    </li>)}</ul>
  </div>;
}

function AgentMap({ scene, plugins, ui, setUi }: { scene: AgentScene; plugins: SpacePlugin[]; ui: MapUi; setUi: Dispatch<SetStateAction<MapUi>> }) {
  const flow = useReactFlow();
  const distant = useStore(isDistant);
  const { selected, pluginId: savedPluginId, query, paused } = ui;
  const selectedPlugin = plugins.find(plugin => plugin.id === savedPluginId);
  const pluginId = selectedPlugin?.id ?? "";
  // A viewport the user moved keeps its zoom and pan when the width changes.
  const userView = useRef(false);
  const markUserView = useCallback(() => { userView.current = true; }, []);
  const [motionAllowed, setMotionAllowed] = useState(false);
  const map = useRef<HTMLDivElement>(null);
  useEffect(() => {
    if (!map.current) return;
    let width = map.current.clientWidth;
    const observer = new ResizeObserver(entries => {
      const next = entries[0]?.contentRect.width;
      if (next === undefined || Math.abs(next - width) < 1) return;
      width = next;
      if (!userView.current) void flow.fitView({ ...initialFit, duration: 0 });
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
  const choose = useCallback((id: string) => setUi(current => ({ ...current, selected: id, pluginId: "" })), [setUi]);
  const setQuery = (next: string) => setUi(current => ({ ...current, query: next }));
  const togglePaused = () => setUi(current => ({ ...current, paused: !current.paused }));
  const focusRegion = useCallback((id: string) => {
    const region = scene.regions.find(item => item.id === id);
    if (!region) return;
    markUserView();
    choose(id);
    void flow.fitBounds(region.bounds, { padding: .16, duration });
  }, [flow, duration, choose, scene, markUserView]);
  const focusAgent = (id: string) => {
    const agent = scene.agents.find(item => item.id === id);
    if (!agent) return;
    markUserView();
    setUi(current => ({ ...current, selected: id, pluginId: "", query: "" }));
    void flow.setCenter(agent.position.x, agent.position.y + 20, { zoom: 1, duration });
  };
  const members = pluginMembers(scene.agents, plugins.find(plugin => plugin.id === pluginId));
  const memberIds = new Set(members.map(agent => agent.id));
  const term = query.trim().toLocaleLowerCase();
  const matches = scene.agents.filter(agent => `${agent.name} ${agent.id}`.toLocaleLowerCase().includes(term));
  const matchIds = new Set(matches.map(agent => agent.id));
  const nodes: Node[] = [
    ...scene.regions.map((region): RegionNode => ({ id: `region-${region.id}`, type: "region", position: { x: region.bounds.x, y: region.bounds.y },
      width: region.bounds.width, height: region.bounds.height, data: { region, moving: motionAllowed && !paused, axis: scene.axis, focus: focusRegion }, selectable: false, draggable: false, zIndex: distant ? 3 : 0 })),
    ...scene.agents.map((agent): PortraitNode => {
      const parent = scene.agents.find(item => item.id === agent.managerId);
      const children = scene.agents.filter(item => item.managerId === agent.id).length;
      return { id: agent.id, type: "portrait", position: { x: agent.position.x - 90, y: agent.position.y - 65 }, width: 180, height: 180, zIndex: 2,
        data: { agent, active: selected === agent.id && !pluginId, dimmed: !!pluginId && !memberIds.has(agent.id) || !!term && !matchIds.has(agent.id), crowded: scene.agents.length > 6, choose, axis: scene.axis,
          relationship: parent ? `Managed by ${parent.name}` : children ? `Manages ${children} ${children === 1 ? "agent" : "agents"}` : "Independent agent" } };
    }),
  ];
  const fit = () => { userView.current = false; void flow.fitView({ padding: .12, duration, maxZoom: 1.15 }); };
  const selectPlugin = (id: string) => setUi(current => ({ ...current, query: "", pluginId: current.pluginId === id ? "" : id }));
  return <>
    <div className="space-toolbar">
      <div className="space-search"><InputGroup><InputGroupAddon><MagnifyingGlass /></InputGroupAddon>
        <InputGroupInput type="search" aria-label="Find an agent" placeholder="Find an agent…" value={query} onChange={event => setQuery(event.target.value)} onKeyDown={event => {
          if (event.key === "Escape") setQuery("");
          if (event.key === "Enter" && matches.length === 1) focusAgent(matches[0]!.id);
        }} />
        {query && <InputGroupAddon align="inline-end"><InputGroupButton aria-label="Clear search" size="icon-xs" onClick={() => setQuery("")}><X /></InputGroupButton></InputGroupAddon>}
      </InputGroup>{term && <div className="space-search-results"><p role="status">{matches.length ? `${agentCount(matches.length)} matching` : "No matching agents"}</p>{matches.map(agent => <button key={agent.id} onClick={() => focusAgent(agent.id)}>{agent.name}<span>Go to agent</span></button>)}</div>}</div>
      <div className="space-view-controls"><Button variant="outline" onClick={fit}><ArrowsOutSimple />Fit view</Button><Button variant="ghost" size="icon" aria-label={paused ? "Resume map motion" : "Pause map motion"} aria-pressed={paused} onClick={togglePaused}>{paused ? <Play /> : <Pause />}</Button></div>
    </div>
    <div ref={map} className="space-map" data-motion={motionAllowed && !paused} aria-label="Agent relationship map" onFocusCapture={event => {
      const target = event.target;
      if (!(target instanceof HTMLButtonElement || target instanceof HTMLAnchorElement) || !map.current || !(target.dataset.agentId || target.dataset.regionId)) return;
      const item = target.getBoundingClientRect(), frame = map.current.getBoundingClientRect();
      if (item.left >= frame.left && item.right <= frame.right && item.top >= frame.top && item.bottom <= frame.bottom) return;
      const agent = scene.agents.find(candidate => candidate.id === (target.dataset.agentId ?? target.dataset.regionId));
      if (agent) { markUserView(); void flow.setCenter(agent.position.x, agent.position.y, { zoom: flow.getZoom(), duration: 0 }); }
    }}>
      {scene.agents.length ? <ReactFlow nodes={nodes} nodeTypes={spaceNodeTypes} fitView fitViewOptions={initialFit} colorMode="dark"
        minZoom={.12} maxZoom={1.7} nodesDraggable={false} nodesConnectable={false} nodesFocusable={false} edgesFocusable={false} elementsSelectable={false}
        deleteKeyCode={null} selectionKeyCode={null} zoomOnDoubleClick={false} zoomOnScroll zoomOnPinch preventScrolling
        attributionPosition="bottom-left" aria-label="Explore agent spaces" onMoveStart={event => { if (event) markUserView(); }} onPaneClick={() => setUi(current => ({ ...current, query: "", pluginId: "" }))}>
        <Background gap={28} size={.7} color="#ffffff19" />
        {pluginId && members.length > 0 && <ViewportPortal><svg className="space-shared-field" aria-hidden="true"><path d={regionPath(members.map(agent => agent.position), 128)} /></svg>
          <div className="space-capability-label" style={{ left: members.reduce((sum, agent) => sum + agent.position.x, 0) / members.length, top: Math.min(...members.map(agent => agent.position.y)) - 90 }}><Cube size={18} /><span>{selectedPlugin?.name}<small>{agentCount(members.length)} · saved selection</small></span></div>
        </ViewportPortal>}
        <ZoomControls onAdjust={markUserView} />
      </ReactFlow> : <div className="space-map-empty">No agents yet. Agents will appear here when registered on this Host.</div>}
    </div>
    <div className="space-plugins" aria-label="Shared connections"><span><Cube size={16} />Shared connections</span><div>{plugins.map(plugin => {
      const count = pluginMembers(scene.agents, plugin).length;
      return <button key={plugin.id} className="space-plugin" data-active={pluginId === plugin.id} aria-pressed={pluginId === plugin.id} aria-label={`${plugin.name}, ${agentCount(count)}`} onClick={() => selectPlugin(plugin.id)}><i aria-hidden="true" />{plugin.name}<span>{count}</span></button>;
    })}</div></div>
    <div className="space-caption"><span>{pluginId ? `${agentCount(members.length)} select ${selectedPlugin?.name} in their saved configuration.` : plugins.length ? "Open an agent from its portrait. Select a connection to reveal saved selections." : "Open an agent from its portrait. No shared connections are recorded yet."} <a href="#library">Manage capabilities</a></span><span>Drag to pan · Scroll or pinch to zoom</span></div>
  </>;
}

function ZoomControls({ onAdjust }: { onAdjust: () => void }) {
  const flow = useReactFlow();
  const zoom = useStore(state => state.transform[2]);
  return <div className="space-zoom-controls"><Button variant="ghost" size="icon" aria-label="Zoom out" disabled={zoom <= .121} onClick={() => { onAdjust(); void flow.zoomOut(); }}><Minus /></Button><output aria-label="Map zoom">{Math.round(zoom * 100)}%</output><Button variant="ghost" size="icon" aria-label="Zoom in" disabled={zoom >= 1.69} onClick={() => { onAdjust(); void flow.zoomIn(); }}><Plus /></Button></div>;
}
