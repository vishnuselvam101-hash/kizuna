import { useEffect, useRef, useState } from "react";
import { useI18n } from "./i18n";

export type NodeKind = "document" | "claim" | "subject" | "finding" | "decision";
export interface GNode {
  id: string;
  kind: NodeKind;
  label: string;
  tone: "crit" | "ok" | "warn" | "info" | "base";
}
export interface Graph {
  nodes: GNode[];
  edges: [string, string][];
}

const COLS: NodeKind[] = ["document", "claim", "subject", "decision"];
const colOf = (k: NodeKind) => (k === "finding" ? 3 : COLS.indexOf(k));

export function webglUsable(): boolean {
  if (typeof window === "undefined" || typeof WebGLRenderingContext === "undefined") return false;
  if (window.matchMedia?.("(prefers-reduced-motion: reduce)").matches) return false;
  const nav = navigator as Navigator & { connection?: { saveData?: boolean } };
  if (nav.connection?.saveData || (nav.hardwareConcurrency ?? 4) <= 2) return false;
  try {
    return !!document.createElement("canvas").getContext("webgl2");
  } catch {
    return false;
  }
}

/** Static, keyboard-operable SVG rendering of the same graph (reduced motion / no WebGL / low power). */
export function GraphSvg({ g, label, onPick, height = 320 }: { g: Graph; label: string; onPick: (id: string) => void; height?: number }) {
  const W = 1000;
  const per = new Map<number, GNode[]>();
  g.nodes.forEach((n) => per.set(colOf(n.kind), [...(per.get(colOf(n.kind)) ?? []), n]));
  const pos = new Map<string, { x: number; y: number }>();
  per.forEach((ns, c) => {
    const shown = ns.slice(0, 40);
    shown.forEach((n, i) => pos.set(n.id, { x: 80 + c * ((W - 160) / 3), y: ((i + 1) * height) / (shown.length + 1) }));
  });
  return (
    <svg className="graph-svg" viewBox={`0 0 ${W} ${height}`} role="group" aria-label={label} preserveAspectRatio="xMidYMid meet">
      {g.edges.map(([a, b], i) => {
        const pa = pos.get(a), pb = pos.get(b);
        return pa && pb ? <line key={i} x1={pa.x} y1={pa.y} x2={pb.x} y2={pb.y} className="g-edge" /> : null;
      })}
      {g.nodes.map((n) => {
        const p = pos.get(n.id);
        if (!p) return null;
        return (
          <g
            key={n.id}
            className={`g-node ${n.tone}`}
            transform={`translate(${p.x} ${p.y})`}
            role="button"
            tabIndex={0}
            aria-label={n.label}
            onClick={() => onPick(n.id)}
            onKeyDown={(e) => (e.key === "Enter" || e.key === " ") && (e.preventDefault(), onPick(n.id))}
          >
            <rect x={-7} y={-7} width={14} height={14} transform="rotate(45)" />
            <text x={14} y={4}>
              {n.label.length > 22 ? `${n.label.slice(0, 21)}…` : n.label}
            </text>
          </g>
        );
      })}
    </svg>
  );
}

/** WebGL graph (lazy three.js chunk) with SVG fallback. */
export function GraphScene({ g, label, onPick, height = 360 }: { g: Graph; label: string; onPick: (id: string) => void; height?: number }) {
  const ref = useRef<HTMLCanvasElement>(null);
  const [gl] = useState(webglUsable);
  const [failed, setFailed] = useState(false);
  useEffect(() => {
    if (!gl || !ref.current) return;
    let dispose: (() => void) | undefined;
    let dead = false;
    import("./scene3d")
      .then((m) => {
        if (!dead && ref.current) dispose = m.mountGraph(ref.current, g, onPick);
      })
      .catch(() => setFailed(true));
    return () => {
      dead = true;
      dispose?.();
    };
    // Rebuild only when the graph identity changes.
  }, [gl, g]);
  return (
    <figure className="graph">
      {!gl || failed ? (
        <GraphSvg g={g} label={label} onPick={onPick} height={height} />
      ) : (
        <div className="graph-gl" style={{ height }}>
          <canvas ref={ref} aria-label={label} role="img" />
        </div>
      )}
      <Legend g={g} />
    </figure>
  );
}

const TONES = ["crit", "warn", "ok", "info", "base"] as const;
function Legend({ g }: { g: Graph }) {
  const { t } = useI18n();
  const present = TONES.filter((x) => g.nodes.some((n) => n.tone === x));
  return (
    <ul className="legend" aria-label={t("graph.legend")}>
      {present.map((x) => (
        <li key={x} className={x}>
          {t(`graph.tone.${x}`)}
        </li>
      ))}
    </ul>
  );
}
