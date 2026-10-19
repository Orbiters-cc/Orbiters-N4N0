import { motion } from "motion/react";
import { useEffect, useMemo, useRef, useState } from "react";
import { useLive } from "../lib/live";

interface View {
  cx: number;
  cy: number;
  span: number;
}

/**
 * Top-down map of the camera path (Unity X right, Z up the screen). The view eases towards the path's
 * bounds so new points glide in instead of jumping (the frame loop runs only while it moves); the current
 * position carries a heading cone.
 */
export function TrailMap({ points, heading, live, className }: { points: [number, number, number][]; heading?: number | null; live?: boolean; className?: string }) {
  const windowLive = useLive();
  const box = useRef<HTMLDivElement>(null);
  const [aspect, setAspect] = useState(2.5);
  useEffect(() => {
    const el = box.current;
    if (!el) return;
    const ro = new ResizeObserver(([e]) => {
      const r = e.contentRect;
      if (r.width > 0 && r.height > 0) setAspect(r.width / r.height);
    });
    ro.observe(el);
    return () => ro.disconnect();
  }, []);

  const target = useMemo<View>(() => {
    if (points.length === 0) return { cx: 0, cy: 0, span: 10 };
    let minX = Infinity,
      maxX = -Infinity,
      minZ = Infinity,
      maxZ = -Infinity;
    for (const [x, , z] of points) {
      minX = Math.min(minX, x);
      maxX = Math.max(maxX, x);
      minZ = Math.min(minZ, z);
      maxZ = Math.max(maxZ, z);
    }
    // Span is the visible width: fit the path's width and its depth at the box's aspect ratio, with a margin.
    const span = Math.max(6, (maxX - minX) * 1.25, (maxZ - minZ) * 1.25 * aspect);
    return { cx: (minX + maxX) / 2, cy: -(minZ + maxZ) / 2, span };
  }, [points, aspect]);

  const [view, setView] = useState<View>(target);
  const viewRef = useRef(view);
  useEffect(() => {
    let raf = 0;
    const step = () => {
      const v = viewRef.current;
      const k = 0.12;
      const next = { cx: v.cx + (target.cx - v.cx) * k, cy: v.cy + (target.cy - v.cy) * k, span: v.span + (target.span - v.span) * k };
      const settled = Math.abs(next.span - target.span) < 0.001 && Math.abs(next.cx - target.cx) < 0.001 && Math.abs(next.cy - target.cy) < 0.001;
      viewRef.current = settled ? target : next;
      setView(viewRef.current);
      if (!settled) raf = requestAnimationFrame(step);
    };
    if (!windowLive) {
      viewRef.current = target;
      setView(target);
      return;
    }
    raf = requestAnimationFrame(step);
    return () => cancelAnimationFrame(raf);
  }, [target, windowLive]);

  const w = view.span;
  const h = view.span / aspect;
  const x0 = view.cx - w / 2;
  const y0 = view.cy - h / 2;
  const d = points.length > 1 ? "M" + points.map(([x, , z]) => `${x.toFixed(2)} ${(-z).toFixed(2)}`).join("L") : "";
  const last = points[points.length - 1];
  const grid = w > 60 ? 10 : w > 20 ? 5 : 1;
  const lines: number[] = [];
  for (let gx = Math.floor(x0 / grid) * grid; gx < x0 + w; gx += grid) lines.push(gx);
  const hlines: number[] = [];
  for (let gy = Math.floor(y0 / grid) * grid; gy < y0 + h; gy += grid) hlines.push(gy);
  const unit = Math.min(w, h * 1.6);
  const stroke = unit / 300;

  return (
    <div ref={box} className={`relative overflow-hidden rounded-[var(--radius-tile)] bg-black/25 hairline ${className ?? ""}`}>
      <svg viewBox={`${x0} ${y0} ${w} ${h}`} preserveAspectRatio="xMidYMid meet" className="absolute inset-0 size-full">
        <defs>
          <linearGradient id="trail" x1="0" y1="0" x2="1" y2="0">
            <stop offset="0" stopColor="#48e6ff" />
            <stop offset="0.55" stopColor="#7466ff" />
            <stop offset="1" stopColor="#bd65ff" />
          </linearGradient>
          <radialGradient id="cone">
            <stop offset="0" stopColor="#48e6ff" stopOpacity="0.55" />
            <stop offset="1" stopColor="#48e6ff" stopOpacity="0" />
          </radialGradient>
        </defs>
        {lines.map((gx) => (
          <line key={`v${gx}`} x1={gx} x2={gx} y1={y0} y2={y0 + h} stroke="rgba(255,255,255,0.045)" strokeWidth={stroke} />
        ))}
        {hlines.map((gy) => (
          <line key={`h${gy}`} y1={gy} y2={gy} x1={x0} x2={x0 + w} stroke="rgba(255,255,255,0.045)" strokeWidth={stroke} />
        ))}
        {d && (
          <>
            <path d={d} fill="none" stroke="url(#trail)" strokeWidth={stroke * 6} strokeLinecap="round" strokeLinejoin="round" opacity={0.18} />
            <path d={d} fill="none" stroke="url(#trail)" strokeWidth={stroke * 2.2} strokeLinecap="round" strokeLinejoin="round" />
          </>
        )}
        {last && (
          <g transform={`translate(${last[0]} ${-last[2]})`}>
            {heading != null && (
              <path d={`M0 0 L${-unit * 0.05} ${-unit * 0.11} A ${unit * 0.12} ${unit * 0.12} 0 0 1 ${unit * 0.05} ${-unit * 0.11} Z`} fill="url(#cone)" transform={`rotate(${heading})`} />
            )}
            {live && windowLive && (
              <motion.circle r={unit * 0.02} fill="#48e6ff" animate={{ r: [unit * 0.012, unit * 0.04], opacity: [0.6, 0] }} transition={{ duration: 1.6, repeat: Infinity, ease: "easeOut" }} />
            )}
            <circle r={unit * 0.0115} fill="#ffffff" stroke="#48e6ff" strokeWidth={stroke * 1.4} />
          </g>
        )}
      </svg>
      {points.length === 0 && (
        <div className="absolute inset-0 grid place-items-center text-[12.5px] text-fg-4">The camera path appears here while you record</div>
      )}
      <div className="absolute bottom-2.5 left-3 flex items-center gap-2 text-[10.5px] text-fg-4">
        <span className="inline-block h-px w-8 bg-fg-4" />
        {grid} m
      </div>
    </div>
  );
}
