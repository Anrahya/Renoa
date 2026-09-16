import { useEffect, useState, type RefObject } from "react";

interface Curve { id: string; path: string; x: number; y: number; endX: number; endY: number }

// These curves encode actual component-selection relationships between DOM anchors.
// They are data visualization, not raster artwork or a model-execution animation.
export function ProfileConnections({ stage }: { stage: RefObject<HTMLDivElement | null> }) {
  const [curves, setCurves] = useState<Curve[]>([]);
  useEffect(() => {
    const root = stage.current;
    if (!root) return;
    const draw = () => {
      const origin = root.getBoundingClientRect();
      const agent = root.querySelector("[data-profile-anchor]")?.getBoundingClientRect();
      if (!agent || root.clientWidth < 900) { setCurves([]); return; }
      const centerX = agent.left - origin.left + agent.width / 2;
      const centerY = agent.top - origin.top + agent.height / 2;
      const radius = agent.width / 2 + 14;
      setCurves(Array.from(root.querySelectorAll<HTMLElement>("[data-component-anchor]")).map(element => {
        const box = element.getBoundingClientRect();
        const id = element.dataset.componentAnchor ?? "";
        const right = id === "current";
        const x = (right ? box.left - 24 : box.right + 24) - origin.left;
        const y = box.top + 17 - origin.top;
        const angle = Math.atan2(y - centerY, x - centerX);
        const endX = centerX + Math.cos(angle) * radius;
        const endY = centerY + Math.sin(angle) * radius;
        const bend = Math.abs(endX - x) * .6;
        const path = `M ${x} ${y} C ${x + (right ? -bend : bend)} ${y}, ${endX + (right ? bend : -bend)} ${endY}, ${endX} ${endY}`;
        return { id, path, x, y, endX, endY };
      }));
    };
    const observer = new ResizeObserver(draw);
    observer.observe(root);
    root.querySelectorAll("[data-component-anchor], [data-profile-anchor], .ap-identity").forEach(element => observer.observe(element));
    void document.fonts.ready.then(draw);
    draw();
    return () => observer.disconnect();
  }, [stage]);
  return <svg className="ap-connections" aria-hidden="true">{curves.map(curve => <g key={curve.id}>
    <path d={curve.path} /><circle cx={curve.x} cy={curve.y} r="3.1" /><circle cx={curve.endX} cy={curve.endY} r="3.1" />
  </g>)}</svg>;
}
