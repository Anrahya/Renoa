import { memo, useEffect, useId, useMemo, useRef } from "react";

// Warp an existing contour without changing its command sequence, so SVG can
// interpolate continuously. The final frame is exactly the first one.
function liquidFrames(path: string, phase: number): string {
  const frames = Array.from({ length: 4 }, (_, step) => {
    const t = step * Math.PI / 2;
    return path.replace(/(-?[\d.]+(?:e[-+]?\d+)?),(-?[\d.]+(?:e[-+]?\d+)?)/g, (_, rawX: string, rawY: string) => {
      const x = Number(rawX), y = Number(rawY);
      const dx = 18 * Math.sin(y / 100 + t + phase) * Math.cos(x / 170 + t);
      const dy = 15 * Math.cos(x / 125 - t + phase) * Math.sin(y / 160 + t);
      return `${(x + dx).toFixed(2)},${(y + dy).toFixed(2)}`;
    });
  });
  return [...frames, frames[0]].join(";");
}

export const LiquidRegion = memo(function LiquidRegion({ path, width, height, identity, moving }: {
  path: string; width: number; height: number; identity: string; moving: boolean;
}) {
  const id = useId().replace(/:/g, "");
  const svg = useRef<SVGSVGElement>(null);
  const phase = useMemo(() => [...identity].reduce((sum, character) => sum + character.charCodeAt(0), 0) % 11, [identity]);
  const frames = useMemo(() => liquidFrames(path, phase), [path, phase]);
  useEffect(() => {
    if (moving) svg.current?.unpauseAnimations();
    else svg.current?.pauseAnimations();
  }, [moving]);
  return <svg ref={svg} viewBox={`0 0 ${width} ${height}`} aria-hidden="true" className="space-contours" style={{ animationDelay: `${-phase}s` }}>
    <defs>
      <path id={`${id}-shape`} d={path}>
        <animate attributeName="d" values={frames} dur={`${10 + phase * .45}s`} repeatCount="indefinite" calcMode="spline" keyTimes="0;.25;.5;.75;1" keySplines=".45 0 .55 1;.45 0 .55 1;.45 0 .55 1;.45 0 .55 1" />
      </path>
      <clipPath id={`${id}-clip`}><use href={`#${id}-shape`} /></clipPath>
      <radialGradient id={`${id}-light`}>
        <stop stopColor="currentColor" stopOpacity=".3" /><stop offset=".55" stopColor="currentColor" stopOpacity=".1" /><stop offset="1" stopColor="currentColor" stopOpacity="0" />
      </radialGradient>
    </defs>
    <use className="space-region-fill" href={`#${id}-shape`} />
    <g clipPath={`url(#${id}-clip)`}>
      <ellipse cx={width * .3} cy={height * .4} rx={width * .5} ry={height * .5} fill={`url(#${id}-light)`}>
        <animateTransform attributeName="transform" type="translate" values="-25 16;45 -24;-25 16" dur={`${9 + phase * .3}s`} repeatCount="indefinite" calcMode="spline" keyTimes="0;.5;1" keySplines=".42 0 .58 1;.42 0 .58 1" />
      </ellipse>
      <ellipse cx={width * .8} cy={height * .7} rx={width * .4} ry={height * .45} fill={`url(#${id}-light)`} opacity=".65">
        <animateTransform attributeName="transform" type="translate" values="18 -22;-42 26;18 -22" dur={`${12 + phase * .4}s`} repeatCount="indefinite" calcMode="spline" keyTimes="0;.5;1" keySplines=".42 0 .58 1;.42 0 .58 1" />
      </ellipse>
    </g>
    <use className="space-contour space-contour-outer" href={`#${id}-shape`} />
    <use className="space-contour space-contour-inner" href={`#${id}-shape`} />
    <use className="space-current" href={`#${id}-shape`} />
  </svg>;
});
