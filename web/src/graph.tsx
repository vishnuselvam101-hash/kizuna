import { useEffect, useId, useMemo, useRef, useState } from "react";
import { GraphModel, KINDS, streamPages, TONES as MODEL_TONES, type PageFetcher } from "./graphmodel";
import type { GraphView } from "./scene3d";
import { Loading, Notice } from "./ui";
import { useI18n } from "./i18n";

export type NodeKind = "document" | "claim" | "subject" | "finding" | "decision";
export interface GNode {
  id: string;
  kind: NodeKind;
  label: string;
  tone: "crit" | "ok" | "warn" | "info" | "base";
  /** What-if marking (simulation graphs): changed evidence, affected or unaffected decisions. */
  mark?: "changed" | "affected" | "unaffected";
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
type GraphProps = { g: Graph; label: string; onPick: (id: string) => void; height?: number; highlight?: ReadonlySet<string> };

/** Nodes a highlight set covers (ids in the set, or nodes marked by a simulation). */
export function highlighted(g: Graph | GraphModel, highlight?: ReadonlySet<string>): Set<string> {
  if (g instanceof GraphModel) {
    const out = new Set<string>();
    highlight?.forEach((id) => g.indexOf(id) !== undefined && out.add(id));
    for (let i = 0; i < g.size; i++) if (g.mark(i) === "affected" || g.mark(i) === "changed") out.add(g.ids[i]);
    return out;
  }
  return new Set(g.nodes.filter((n) => highlight?.has(n.id) || n.mark === "affected" || n.mark === "changed").map((n) => n.id));
}

export function GraphSvg({ g, label, onPick, height = 320, highlight }: GraphProps) {
  const { t } = useI18n();
  const hl = highlighted(g, highlight);
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
        const on = hl.has(a) && hl.has(b);
        return pa && pb ? <line key={i} x1={pa.x} y1={pa.y} x2={pb.x} y2={pb.y} className={on ? "g-edge hl" : "g-edge"} /> : null;
      })}
      {g.nodes.map((n) => {
        const p = pos.get(n.id);
        if (!p) return null;
        return (
          <g
            key={n.id}
            className={`g-node ${n.tone}${hl.size ? (hl.has(n.id) ? " hl" : " dim") : ""}`}
            transform={`translate(${p.x} ${p.y})`}
            role="button"
            tabIndex={0}
            aria-label={hl.has(n.id) ? `${n.label} — ${t("whatif.affected")}` : n.label}
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

/** Above this many nodes the non-WebGL fallback is a table instead of the SVG drawing. */
export const SVG_MAX = 400;

type SceneProps = {
  /** A whole graph, or `pages` to stream it from a paged endpoint. */
  g?: Graph;
  pages?: PageFetcher;
  label: string | ((m: GraphModel) => string);
  onPick: (id: string) => void;
  height?: number;
  highlight?: ReadonlySet<string>;
};

type Mode = "webgl" | "svg" | "table";

/**
 * Evidence graph: WebGL (lazy three.js chunk, layout in a worker) when available and motion is
 * allowed; otherwise the SVG drawing for small graphs and a paged table for large ones. Pages
 * stream in progressively; a keyboard-operable details panel shows the selected node.
 */
export function GraphScene({ g, pages, label, onPick, height = 360, highlight }: SceneProps) {
  const { t, fmtNum } = useI18n();
  const ref = useRef<HTMLCanvasElement>(null);
  const [gl] = useState(webglUsable);
  const [failed, setFailed] = useState(false);
  const staticModel = useMemo(() => (g ? GraphModel.of(g) : null), [g]);
  const [streamed, setStreamed] = useState<GraphModel | null>(null);
  const model = staticModel ?? streamed;
  const [, setVersion] = useState(0);
  const [error, setError] = useState<unknown>(null);
  const [sel, setSel] = useState<number | null>(null);
  const [hover, setHover] = useState<{ i: number; x: number; y: number } | null>(null);
  const view = useRef<GraphView | null>(null);
  const helpId = useId();

  useEffect(() => {
    setSel(null);
    if (g || !pages) return;
    const m = new GraphModel();
    const ctl = new AbortController();
    let shown = false;
    streamPages(m, pages, ctl.signal, () => {
      if (!shown) (shown = true), setStreamed(m);
      setVersion((v) => v + 1);
    }).catch((e) => !ctl.signal.aborted && setError(e));
    return () => ctl.abort();
  }, [g, pages]);

  const mode: Mode | null = !model ? null : gl && !failed ? "webgl" : Math.max(model.expected, model.size) <= SVG_MAX ? "svg" : "table";

  useEffect(() => {
    if (mode !== "webgl" || !model || !ref.current) return;
    let dead = false;
    const canvas = ref.current;
    import("./scene3d")
      .then((m) => {
        if (dead) return;
        view.current = m.mountGraph(
          canvas,
          model,
          {
            onSelect: (i) => setSel(i),
            onOpen: (i) => onPickRef.current(model.ids[i]),
            onHover: (i, x, y) => setHover(i === null ? null : { i, x, y }),
          },
          highlighted(model, highlight),
          !window.matchMedia?.("(prefers-reduced-motion: reduce)").matches,
        );
      })
      .catch(() => setFailed(true));
    return () => {
      dead = true;
      view.current?.dispose();
      view.current = null;
    };
  }, [mode, model, highlight]);
  const onPickRef = useRef(onPick);
  onPickRef.current = onPick;

  const choose = (i: number | null, focus = false) => {
    setSel(i);
    view.current?.select(i, focus);
  };
  const order = useRef<{ m: GraphModel; n: number; list: number[] } | null>(null);
  const step = (dir: 1 | -1) => {
    if (!model) return;
    if (order.current?.m !== model || order.current.n !== model.size) order.current = { m: model, n: model.size, list: notableOrder(model) };
    const list = order.current.list;
    if (!list.length) return;
    const k = sel === null ? -1 : list.indexOf(sel);
    choose(list[(k + dir + list.length) % list.length], true);
  };
  const onKey = (e: React.KeyboardEvent) => {
    const v = view.current;
    const act: Record<string, () => void> = {
      ArrowLeft: () => v?.orbit(-0.12, 0),
      ArrowRight: () => v?.orbit(0.12, 0),
      ArrowUp: () => v?.orbit(0, 0.1),
      ArrowDown: () => v?.orbit(0, -0.1),
      "+": () => v?.zoom(0.8),
      "=": () => v?.zoom(0.8),
      "-": () => v?.zoom(1.25),
      "0": () => v?.reset(),
      n: () => step(1),
      p: () => step(-1),
      Enter: () => sel !== null && model && onPick(model.ids[sel]),
      Escape: () => choose(null),
    };
    const f = act[e.key.length === 1 ? e.key.toLowerCase() : e.key];
    if (f) (e.preventDefault(), f());
  };

  const name = model ? (typeof label === "function" ? label(model) : label) : typeof label === "string" ? label : "";
  const loading = !!model && !model.done;
  return (
    <figure className="graph">
      <Notice error={error} />
      {!model && !error && <Loading on />}
      {model && loading && (
        <p className="muted graph-progress" role="status">
          {t("graph.loading", { n: fmtNum(model.size), total: fmtNum(Math.max(model.expected, model.size)) })}
        </p>
      )}
      {mode === "svg" && model && <GraphSvg g={g ?? model.toGraph()} label={name} onPick={onPick} height={height} highlight={highlight} />}
      {mode === "table" && model && <GraphTable m={model} label={name} onPick={onPick} />}
      {mode === "webgl" && model && (
        <>
          <div className="graph-gl" style={{ height }}>
            <canvas ref={ref} tabIndex={0} role="application" aria-roledescription="graph" aria-label={name} aria-describedby={helpId} onKeyDown={onKey} />
            {hover && hover.i < model.size && (
              <span className="graph-hover" aria-hidden="true" style={{ left: hover.x + 12, top: hover.y + 12 }}>
                {model.labels[hover.i]}
              </span>
            )}
          </div>
          <p id={helpId} className="muted graph-keys">
            {t("graph.keys")}
          </p>
          <NodeDetails m={model} i={sel} onPick={onPick} onSelect={(j) => choose(j, true)} />
        </>
      )}
      {model && <Legend m={model} />}
    </figure>
  );
}

const TONE_ORDER = ["crit", "warn", "ok", "info", "base"];
/** Keyboard stepping order: conflicts and changed items first, then by kind and arrival. */
function notableOrder(m: GraphModel): number[] {
  const idx = Array.from({ length: m.size }, (_, i) => i);
  const rank = (i: number) => (m.mark(i) === "affected" || m.mark(i) === "changed" ? -1 : TONE_ORDER.indexOf(m.tone(i)));
  return idx.sort((a, b) => rank(a) - rank(b) || m.kinds[b] - m.kinds[a] || a - b).slice(0, 5000);
}

function NodeDetails({ m, i, onPick, onSelect }: { m: GraphModel; i: number | null; onPick: (id: string) => void; onSelect: (i: number) => void }) {
  const { t, fmtNum } = useI18n();
  if (i === null || i >= m.size)
    return (
      <div className="graph-detail muted" role="region" aria-label={t("graph.details")} aria-live="polite">
        {t("graph.selectHint")}
      </div>
    );
  const nb = m.neighbours(i, 13);
  const deg = m.degree(i);
  return (
    <div className="graph-detail" role="region" aria-label={t("graph.details")} aria-live="polite">
      <p>
        <b>{m.labels[i]}</b> <span className="muted">{t(`graph.kind.${m.kind(i)}`)}</span> <span className={`g-tone ${m.tone(i)}`}>{t(`graph.tone.${m.tone(i)}`)}</span>{" "}
        <span className="muted">{t("graph.links", { n: fmtNum(deg) })}</span>{" "}
        <button type="button" onClick={() => onPick(m.ids[i])}>
          {t("graph.open")}
        </button>
      </p>
      {nb.length > 0 && (
        <ul className="graph-nb" aria-label={t("graph.linked")}>
          {nb.slice(0, 12).map((j) => (
            <li key={j}>
              <button type="button" className="link" onClick={() => onSelect(j)}>
                {m.labels[j]} <span className="muted">· {t(`graph.kind.${m.kind(j)}`)}</span>
              </button>
            </li>
          ))}
          {deg > 12 && <li className="muted">{t("graph.more", { n: fmtNum(deg - 12) })}</li>}
        </ul>
      )}
    </div>
  );
}

const ROWS = 50;
/** Accessible fallback for graphs too large for the SVG drawing (no WebGL or reduced motion). */
export function GraphTable({ m, label, onPick }: { m: GraphModel; label: string; onPick: (id: string) => void }) {
  const { t, fmtNum } = useI18n();
  const [kind, setKind] = useState<string>("");
  const [q, setQ] = useState("");
  const [page, setPage] = useState(0);
  const rows = useMemo(() => {
    const k = kind ? KINDS.indexOf(kind as NodeKind) : -1;
    const needle = q.trim().toLowerCase();
    const out: number[] = [];
    for (let i = 0; i < m.size; i++) if ((k < 0 || m.kinds[i] === k) && (!needle || m.labels[i].toLowerCase().includes(needle))) out.push(i);
    return out;
    // m.size changes as pages stream in.
  }, [m, m.size, kind, q]);
  const from = Math.min(page * ROWS, Math.max(0, rows.length - 1));
  const shown = rows.slice(from, from + ROWS);
  return (
    <section className="graph-table" aria-label={label}>
      <p className="muted">{t("graph.tableLead", { n: fmtNum(m.size) })}</p>
      <div className="graph-filters">
        <label>
          {t("graph.filterKind")}{" "}
          <select value={kind} onChange={(e) => (setKind(e.target.value), setPage(0))}>
            <option value="">{t("graph.allKinds")}</option>
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {t(`graph.kind.${k}`)}
              </option>
            ))}
          </select>
        </label>{" "}
        <label>
          {t("graph.search")} <input type="search" value={q} onChange={(e) => (setQ(e.target.value), setPage(0))} />
        </label>
      </div>
      <div className="table-wrap" tabIndex={0} role="region" aria-label={t("graph.table")}>
        <table>
          <caption className="sr-only">{label}</caption>
          <thead>
            <tr>
              <th scope="col">{t("graph.label")}</th>
              <th scope="col">{t("graph.kind")}</th>
              <th scope="col">{t("graph.status")}</th>
              <th scope="col">{t("common.actions")}</th>
            </tr>
          </thead>
          <tbody>
            {shown.map((i) => (
              <tr key={i}>
                <td>{m.labels[i]}</td>
                <td>{t(`graph.kind.${m.kind(i)}`)}</td>
                <td>{t(`graph.tone.${m.tone(i)}`)}</td>
                <td>
                  <button type="button" className="link" onClick={() => onPick(m.ids[i])}>
                    {t("graph.open")}
                  </button>
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <p className="graph-pager">
        <span role="status">{t("graph.pageOf", { from: fmtNum(rows.length ? from + 1 : 0), to: fmtNum(from + shown.length), n: fmtNum(rows.length) })}</span>{" "}
        <button type="button" disabled={from === 0} onClick={() => setPage(Math.max(0, page - 1))}>
          {t("graph.prev")}
        </button>{" "}
        <button type="button" disabled={from + ROWS >= rows.length} onClick={() => setPage(page + 1)}>
          {t("graph.next")}
        </button>
      </p>
    </section>
  );
}

const TONES = ["crit", "warn", "ok", "info", "base"] as const;
function Legend({ m }: { m: GraphModel }) {
  const { t } = useI18n();
  const seen = new Set<number>();
  for (let i = 0; i < m.size && seen.size < TONES.length; i++) seen.add(m.tones[i]);
  const present = TONES.filter((x) => seen.has(MODEL_TONES.indexOf(x)));
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
