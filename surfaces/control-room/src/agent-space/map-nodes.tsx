import { memo, useMemo, type CSSProperties } from "react";
import { useStore, type Node, type NodeProps } from "@xyflow/react";
import { CircleNotch, WarningCircle } from "@phosphor-icons/react";
import { agentHref } from "../host-presentation";
import { portraitForAgent } from "../host-identity";
import { stateLabel, type AgentState, type DayMark } from "../host-state";
import { regionPath, type AgentRegion, type DayAxis, type PlacedAgent } from "./scene";
import { LiquidRegion } from "./liquid-region";

export type PortraitNode = Node<{ agent: PlacedAgent; active: boolean; dimmed: boolean; crowded: boolean; relationship: string; axis: DayAxis; choose: (id: string) => void }, "portrait">;
export type RegionNode = Node<{ region: AgentRegion; moving: boolean; axis: DayAxis; focus: (id: string) => void }, "region">;
export const isDistant = (state: { transform: [number, number, number] }) => state.transform[2] < .42;

/** Today's recorded work for one agent on the scene's shared axis. */
export function DayTrack({ marks, axis, className }: { marks: DayMark[]; axis: DayAxis; className: string }) {
  if (!axis || !marks.length) return null;
  const span = Math.max(1, axis.end - axis.start);
  return <span className={className} aria-hidden="true">
    {marks.map((mark, index) => <i key={index} data-state={mark.state}
      style={{ left: `${Math.min(100, Math.max(0, (mark.at - axis.start) / span * 100))}%` }} />)}
  </span>;
}

export const AgentPortrait = memo(function AgentPortrait({ data }: NodeProps<PortraitNode>) {
  const distant = useStore(isDistant);
  const zoom = useStore(state => state.transform[2]);
  const { agent, active, dimmed, relationship, choose } = data;
  // A question waiting on the owner is gold; only broken work is red. Assigned
  // work in progress is a quiet green, and an idle agent says nothing at all.
  const state = agent.state;
  const spoken = state === "needs-you" || state === "failed" ? agent.summary?.status ?? stateLabel[state] : relationship;
  const day = agent.summary?.day ?? [];
  const Target = agent.synthetic ? "button" : "a";
  return <Target href={agent.synthetic ? undefined : agentHref(agent.id)} className="space-agent nodrag nopan" style={{ "--label-scale": Math.max(1, 1 / zoom) } as CSSProperties} data-agent-id={agent.id} data-selected={active} data-dimmed={dimmed} data-distant={distant} data-compact={data.crowded && zoom < .65} data-state={state} aria-pressed={agent.synthetic ? active : undefined}
    tabIndex={distant ? -1 : 0} aria-label={`${agent.synthetic ? "Focus example agent" : "Open"} ${agent.name}. ${relationship}${state === "needs-you" || state === "failed" ? `. ${spoken}` : ""}${day.length ? `. ${day.length} recorded today` : ""}`} onClick={() => choose(agent.id)} title={agent.synthetic ? "Example agent — no Host records" : `Open ${agent.name}`}>
    <span className="space-portrait"><img src={portraitForAgent(agent.id, agent.originalName)} alt="" draggable={false} />
      {state === "needs-you" && <span className="space-attention" aria-hidden="true"><CircleNotch weight="fill" /></span>}
      {state === "failed" && <span className="space-attention" aria-hidden="true"><WarningCircle weight="fill" /></span>}
    </span>
    <strong title={agent.name}>{agent.name}</strong>
    <DayTrack marks={day} axis={data.axis} className="space-agent-day" />
    <span className="space-agent-state" data-state={state}>{spoken}</span>
  </Target>;
});

export const ManagementRegion = memo(function ManagementRegion({ data }: NodeProps<RegionNode>) {
  const distant = useStore(isDistant);
  const zoom = useStore(state => state.transform[2]);
  const { region, focus } = data;
  const path = useMemo(() => regionPath(region.members.map(agent => ({ x: agent.position.x - region.bounds.x, y: agent.position.y - region.bounds.y }))), [region]);
  const urgent = (["needs-you", "failed"] as AgentState[]).filter(key => region.counts[key] > 0);
  const spoken = urgent.map(key => `${region.counts[key]} ${stateLabel[key].toLowerCase()}`).join(", ");
  const day = region.members.flatMap(agent => agent.summary?.day ?? []);
  const active = region.counts["needs-you"] + region.counts.failed + region.counts.running;
  return <div className="space-region" data-state={region.state} style={{ "--region-color": region.color, width: region.bounds.width, height: region.bounds.height } as CSSProperties}>
    <LiquidRegion path={path} width={region.bounds.width} height={region.bounds.height} identity={region.id} moving={data.moving} />
    <button className="space-region-label nodrag nopan" data-region-id={region.id} data-distant={distant} data-far={zoom < .2} data-independent={region.members.length === 1} onClick={() => focus(region.id)}
      style={{ transform: `scale(${Math.max(1, 1 / zoom)})`, transformOrigin: "top left" }}
      aria-label={`Focus ${region.name}’s space, ${region.members.length} ${region.members.length === 1 ? "agent" : "agents"}${urgent.length ? `, ${spoken}` : ""}${day.length ? `, ${day.length} recorded today` : ""}`}>
      <span className="space-region-name">{region.members.length > 1 ? `${region.name}’s space` : "Independent"}</span>
      <span className="space-region-meta">{distant
        ? <>{region.members.length === 1 ? region.name : `${region.members.length} agents`}{urgent.length > 0 && <em>{spoken}</em>}</>
        : <>{active > 0 ? `${active} active today` : "Nothing recorded today"}{urgent.length > 0 && <em>{spoken}</em>}</>}</span>
      <DayTrack marks={day} axis={data.axis} className="space-region-day" />
    </button>
  </div>;
});

export const spaceNodeTypes = { portrait: AgentPortrait, region: ManagementRegion };
