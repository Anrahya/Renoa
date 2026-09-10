import { useEffect, useRef } from "react";

// A conceptual form, entirely local. It never consumes Host records or activity.
// Three bundles share a center; their strands are sampled parametric curves.
const TAU = Math.PI * 2;
const STRANDS = 64;
const SAMPLES = 112;
const ANGLES = [-Math.PI / 2, Math.PI / 6, Math.PI * 5 / 6];

interface FormProps {
  selected: number | null;
  paused: boolean;
}

export function SystemForm({ selected, paused }: FormProps) {
  const canvasRef = useRef<HTMLCanvasElement>(null);
  const options = useRef({ selected, paused });
  options.current = { selected, paused };
  const redraw = useRef<(() => void) | null>(null);

  useEffect(() => {
    const canvas = canvasRef.current;
    const context = canvas?.getContext("2d");
    if (!canvas || !context) return;

    let frame = 0;
    let visible = true;
    let previous = 0;
    let time = 0;
    let width = 1;
    let height = 1;
    let tiltX = 0;
    let tiltY = 0;
    let pointerX = 0;
    let pointerY = 0;
    const emphasis = [1, 1, 1];
    const reduced = window.matchMedia("(prefers-reduced-motion: reduce)");

    function paint() {
      if (!canvas || !context) return;
      const scale = Math.min(width / 760, height / 720);
      const pixelRatio = canvas.width / width;
      context.setTransform(pixelRatio, 0, 0, pixelRatio, 0, 0);
      context.clearRect(0, 0, width, height);
      context.translate(width / 2, height / 2);
      context.scale(scale, scale);

      ANGLES.forEach((baseAngle, bundle) => {
        const angle = baseAngle + tiltX * 0.045;
        const directionX = Math.cos(angle);
        const directionY = Math.sin(angle);
        const intensity = emphasis[bundle] ?? 1;
        for (let strand = 0; strand < STRANDS; strand++) {
          const u = strand / (STRANDS - 1);
          const crossAngle = u * TAU;
          const breathing = Math.sin(time * 0.38 + bundle * 1.6) * 4;
          context.beginPath();
          for (let sample = 0; sample <= SAMPLES; sample++) {
            const t = sample / SAMPLES * TAU;
            const spread = Math.sin(t / 2);
            const length = (138 + breathing) * (1 - Math.cos(t));
            const across = Math.sin(t) * (74 + 22 * Math.cos(crossAngle))
              + 26 * Math.sin(crossAngle) * spread;
            const depth = 34 * Math.cos(crossAngle) * spread
              + 10 * Math.sin(t * 2 + time * 0.18);
            const twist = 0.22 * Math.sin(t) + tiltY * 0.06;
            const x = directionX * length - directionY * across;
            const y = directionY * length + directionX * across;
            const projectedX = x + depth * (0.50 + tiltX * 0.1);
            const projectedY = y * 0.98 + depth * twist;
            if (sample === 0) context.moveTo(projectedX, projectedY);
            else context.lineTo(projectedX, projectedY);
          }
          // Thin transparent fibers create depth through their actual overlap.
          const light = 39 + Math.sin(crossAngle) * 14;
          context.strokeStyle = `hsla(228, 76%, ${light}%, ${0.12 + intensity * 0.31})`;
          context.lineWidth = 0.7 + intensity * 0.18;
          context.stroke();
        }
      });
    }

    function tick(timestamp: number) {
      frame = 0;
      if (!visible || document.hidden) { previous = 0; return; }
      const staticMode = options.current.paused || reduced.matches;
      const elapsed = previous === 0 ? 0 : Math.min((timestamp - previous) / 1000, 0.05);
      previous = timestamp;
      const easing = staticMode ? 1 : 1 - Math.exp(-elapsed * 8);
      if (!staticMode) time += elapsed;
      tiltX += ((staticMode ? 0 : pointerX) - tiltX) * easing;
      tiltY += ((staticMode ? 0 : pointerY) - tiltY) * easing;
      for (let i = 0; i < emphasis.length; i++) {
        const target = options.current.selected === null || options.current.selected === i ? 1 : 0.12;
        emphasis[i] = (emphasis[i] ?? 1) + (target - (emphasis[i] ?? 1)) * easing;
      }
      paint();
      if (!staticMode) frame = window.requestAnimationFrame(tick);
    }

    function requestDraw() {
      if (frame === 0) { previous = 0; frame = window.requestAnimationFrame(tick); }
    }

    function resize() {
      if (!canvas) return;
      const bounds = canvas.getBoundingClientRect();
      width = bounds.width;
      height = bounds.height;
      if (width === 0 || height === 0) return;
      // Two physical pixels per CSS pixel preserve fine lines without a large
      // backing store on high-density phones. This is a rendering budget only.
      const ratio = Math.min(window.devicePixelRatio || 1, 2);
      canvas.width = Math.round(width * ratio);
      canvas.height = Math.round(height * ratio);
      paint();
      requestDraw();
    }

    function pointer(event: PointerEvent) {
      if (!canvas || event.pointerType === "touch") return;
      const bounds = canvas.getBoundingClientRect();
      pointerX = (event.clientX - bounds.left) / bounds.width * 2 - 1;
      pointerY = (event.clientY - bounds.top) / bounds.height * 2 - 1;
    }
    function resetPointer() { pointerX = 0; pointerY = 0; }

    const sizeObserver = new ResizeObserver(resize);
    const visibilityObserver = new IntersectionObserver(([entry]) => {
      visible = entry?.isIntersecting ?? false;
      if (visible) requestDraw();
    });
    sizeObserver.observe(canvas);
    visibilityObserver.observe(canvas);
    canvas.addEventListener("pointermove", pointer);
    canvas.addEventListener("pointerleave", resetPointer);
    document.addEventListener("visibilitychange", requestDraw);
    reduced.addEventListener("change", requestDraw);
    redraw.current = requestDraw;
    resize();

    return () => {
      window.cancelAnimationFrame(frame);
      sizeObserver.disconnect();
      visibilityObserver.disconnect();
      canvas.removeEventListener("pointermove", pointer);
      canvas.removeEventListener("pointerleave", resetPointer);
      document.removeEventListener("visibilitychange", requestDraw);
      reduced.removeEventListener("change", requestDraw);
      redraw.current = null;
    };
  }, []);

  useEffect(() => { redraw.current?.(); }, [selected, paused]);

  return <canvas ref={canvasRef} className="renoa-form-canvas" aria-hidden="true" />;
}
