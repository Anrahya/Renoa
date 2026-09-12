import { useEffect, useState, type RefObject } from "react";
import { connectionPath, type Anchor } from "./host-system-model";

export function SystemConnections({ stage, viewport, identity, pulses }: {
  stage: RefObject<HTMLDivElement | null>; viewport: RefObject<HTMLDivElement | null>;
  identity: string; pulses: Map<string, number>;
}) {
  const [paths, setPaths] = useState<{ id: string; d: string }[]>([]);
  // Parent-owned refs are attached after child layout effects on the first mount.
  useEffect(() => {
    const root = stage.current;
    const scroll = viewport.current;
    if (!root || !scroll) return;
    let frame = 0;
    function measure() {
      if (!root) return;
      const bounds = root.getBoundingClientRect();
      const origin = root.querySelector<HTMLElement>("[data-host-anchor]");
      if (!origin) return;
      const anchor = (el: Element): Anchor => { const r = el.getBoundingClientRect(); return { x: r.x - bounds.x, y: r.y - bounds.y, width: r.width, height: r.height }; };
      const from = anchor(origin);
      const compact = getComputedStyle(root).display === "block";
      const view = scroll!.getBoundingClientRect();
      setPaths(Array.from(root.querySelectorAll<HTMLElement>("[data-agent-anchor]")).filter(el => {
        const box = el.getBoundingClientRect();
        // Keep every agent in the inventory, without drawing offscreen branches
        // through the connections currently visible in a large list.
        return box.bottom >= view.top && box.top <= view.bottom;
      }).map(el => ({ id: el.dataset.agentAnchor!, d: connectionPath(from, anchor(el), compact) })));
    }
    const schedule = () => { cancelAnimationFrame(frame); frame = requestAnimationFrame(measure); };
    const observer = new ResizeObserver(schedule);
    observer.observe(root);
    root.querySelectorAll("[data-host-anchor], .system-agent").forEach(el => observer.observe(el));
    scroll.addEventListener("scroll", schedule, { passive: true });
    measure();
    return () => { observer.disconnect(); scroll.removeEventListener("scroll", schedule); cancelAnimationFrame(frame); };
  }, [stage, viewport, identity]);
  return <svg className="system-connections" aria-hidden="true">
    {paths.map(path => <g key={path.id}><path d={path.d} className="system-connection" />
      {pulses.has(path.id) && <path key={pulses.get(path.id)} d={path.d} pathLength="1" className="system-connection-pulse" />}</g>)}
  </svg>;
}
