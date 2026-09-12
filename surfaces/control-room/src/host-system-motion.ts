import { useEffect, useRef, useState, type RefObject } from "react";
import type { HostSnapshot } from "./host-contract";
import { changedAgents } from "./host-system-model";

export function useSystemMotion(stage: RefObject<HTMLDivElement | null>, host: HostSnapshot,
  live: boolean, receivedAt: number | null, paused: boolean) {
  const [visible, setVisible] = useState(false);
  const [reduced, setReduced] = useState(false);
  const [now, setNow] = useState(() => Date.now());
  const [pulses, setPulses] = useState<Map<string, number>>(new Map());
  const previous = useRef<HostSnapshot | null>(null);
  useEffect(() => {
    const preference = window.matchMedia("(prefers-reduced-motion: reduce)");
    let intersects = true;
    const visibility = () => setVisible(document.visibilityState === "visible" && intersects);
    const motion = () => setReduced(preference.matches);
    const observer = new IntersectionObserver(entries => { intersects = entries[0]?.isIntersecting ?? false; visibility(); });
    if (stage.current) observer.observe(stage.current);
    document.addEventListener("visibilitychange", visibility);
    preference.addEventListener("change", motion);
    visibility(); motion();
    return () => { observer.disconnect(); document.removeEventListener("visibilitychange", visibility); preference.removeEventListener("change", motion); };
  }, [stage]);
  // One clock for the whole map, never one timer per agent or schedule.
  useEffect(() => {
    if (!live || !visible || paused) return;
    setNow(Date.now());
    const timer = window.setInterval(() => setNow(Date.now()), 1000);
    return () => clearInterval(timer);
  }, [live, visible, paused]);
  useEffect(() => {
    const sameHost = previous.current?.host_id === host.host_id;
    const enabled = live && visible && !paused && !reduced;
    const changed = enabled ? changedAgents(previous.current, host) : [];
    previous.current = host;
    if (!enabled || !sameHost) setPulses(new Map());
    else if (changed.length) setPulses(new Map(changed.map(id => [id, receivedAt ?? Date.now()])));
  }, [host, live, visible, paused, reduced, receivedAt]);
  // Expiry follows an actual pulse change, not identical polling snapshots.
  useEffect(() => {
    if (!pulses.size) return;
    const timeout = window.setTimeout(() => setPulses(new Map()), 1800);
    return () => clearTimeout(timeout);
  }, [pulses]);
  return { now: live ? now : receivedAt, pulses, moving: live && visible && !paused && !reduced, reduced };
}
