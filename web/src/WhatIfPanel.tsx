import { useEffect, useMemo, useRef, useState, useSyncExternalStore } from "react";
import { claimValue } from "./format";
import { GraphScene, type Graph } from "./graph";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Claim } from "./types";
import { Badge, Notice, short } from "./ui";

// ---- API types (POST /simulations, whatif@1) ------------------------------------------------------------------

export interface SimImpact {
  decision_id: string;
  kind: string | null;
  title: string | null;
  subject_key: string | null;
  status_before: string;
  status_after: string;
  material: boolean;
  cause_kind: string;
  reason: string;
  path: string[];
  semantic_change: Record<string, unknown>;
}

export interface SimDiff {
  appear_total: number;
  clear_total: number;
  changed_total: number;
  appear: Record<string, unknown>[];
  clear: Record<string, unknown>[];
  changed: { before: Record<string, unknown>; after: Record<string, unknown> }[];
}

export interface SimResult {
  simulation: string;
  read_only: boolean;
  change: { type: "supersede" | "withdraw"; before: Claim[]; before_total: number; after: Claim | null; simulated_claim_id: string | null };
  decisions: {
    affected_total: number;
    material_total: number;
    newly_flagged_total: number;
    bounded: boolean;
    reached: number | null;
    limit: number;
    review_batch: boolean;
    truncated: boolean;
    sample: SimImpact[];
  };
  matching: SimDiff;
  integrity: { signals: SimDiff; drift: SimDiff };
  resupply: null | {
    suppliers: { supplier: string; documents: { doc: string; name: string }[]; baseline_count: number }[];
  };
  graph: Graph;
  run_id?: string;
}

export type SimChange =
  | { type: "supersede"; claim_id: string; correction: Record<string, unknown> }
  | { type: "withdraw"; claim_ids?: string[]; artifact_id?: string };

// ---- Active preview (highlights in the trace view) -------------------------------------------------------------

let active: SimResult | null = null;
const listeners = new Set<() => void>();
export const simPreview = {
  set(r: SimResult | null) {
    active = r;
    listeners.forEach((l) => l());
  },
  get: () => active,
  subscribe(l: () => void) {
    listeners.add(l);
    return () => void listeners.delete(l);
  },
};

/** The active what-if preview and the ids it highlights (affected decisions and the changed evidence). */
export function useSimPreview(): { sim: SimResult | null; ids: ReadonlySet<string> } {
  const sim = useSyncExternalStore(simPreview.subscribe, simPreview.get, simPreview.get);
  const ids = useMemo(() => {
    const s = new Set<string>();
    if (!sim) return s;
    sim.change.before.forEach((c) => s.add(c.id));
    sim.decisions.sample.filter((i) => i.material).forEach((i) => s.add(i.decision_id));
    return s;
  }, [sim]);
  return { sim, ids };
}

// ---- Panel -----------------------------------------------------------------------------------------------------

/** What-if panel for one claim (value/spec/revision change or withdrawal) or one document (withdraw all). */
export function WhatIfPanel({ claim, doc, onClose }: { claim?: Claim; doc?: { id: string; name: string }; onClose: () => void }) {
  const { api } = useSession();
  const { t, lang } = useI18n();
  const [mode, setMode] = useState<"supersede" | "withdraw">(claim ? "supersede" : "withdraw");
  const [text, setText] = useState(claim?.value_text ?? "");
  const [num, setNum] = useState(claim?.value_num ?? "");
  const [unit, setUnit] = useState(claim?.value_unit ?? "");
  const [rev, setRev] = useState(claim?.revision ?? "");
  const [reason, setReason] = useState("");
  const [record, setRecord] = useState(false);
  const [busy, setBusy] = useState(false);
  const [err, setErr] = useState<unknown>(null);
  const [res, setRes] = useState<SimResult | null>(null);
  const head = useRef<HTMLHeadingElement>(null);
  const numeric = claim ? claim.value_num != null : false;

  useEffect(() => head.current?.focus(), []);

  // Closing ends the preview; it otherwise stays active so the trace view can highlight it.
  function close() {
    simPreview.set(null);
    onClose();
  }

  function change(): SimChange {
    if (mode === "withdraw" || !claim) return doc && !claim ? { type: "withdraw", artifact_id: doc.id } : { type: "withdraw", claim_ids: [claim!.id] };
    return {
      type: "supersede",
      claim_id: claim.id,
      correction: {
        subject_type: claim.subject_type,
        subject_key: claim.subject_key,
        predicate: claim.predicate,
        value_text: numeric ? null : text || null,
        value_num: numeric ? num || null : null,
        value_unit: numeric ? unit || null : null,
        revision: rev || null,
        scope: claim.scope ?? undefined,
        artifact_id: claim.artifact_id,
        reason: reason.trim() || "what-if",
      },
    };
  }

  async function run(e: React.FormEvent) {
    e.preventDefault();
    setBusy(true);
    setErr(null);
    try {
      const r = await api.request<SimResult>("POST", "/simulations", { change: change(), sample: 100, record });
      setRes(r);
      simPreview.set(r);
    } catch (x) {
      setErr(x);
      setRes(null);
      simPreview.set(null);
    } finally {
      setBusy(false);
    }
  }

  const why = (v: Record<string, unknown>) => String((lang === "ja" ? v.why_ja : v.why) ?? v.why ?? v.kind ?? "");
  const id = claim?.id ?? doc?.id ?? "x";
  return (
    <section
      className="whatif"
      aria-labelledby={`wi-${id}`}
      onKeyDown={(e) => {
        if (e.key === "Escape") {
          e.stopPropagation();
          close();
        }
      }}
    >
      <header className="finding-head">
        <h3 id={`wi-${id}`} ref={head} tabIndex={-1}>
          {t("whatif.title")}
        </h3>
        <button type="button" className="ghost" onClick={close}>
          {t("whatif.close")}
        </button>
      </header>
      <p className="muted">{t("whatif.lead")}</p>
      <form className="whatif-form" onSubmit={run}>
        {claim && (
          <fieldset className="whatif-modes wide">
            <legend>{t("whatif.mode")}</legend>
            {(["supersede", "withdraw"] as const).map((m) => (
              <label key={m}>
                <input type="radio" name={`wi-mode-${id}`} value={m} checked={mode === m} onChange={() => setMode(m)} /> {t(`whatif.mode.${m}`)}
              </label>
            ))}
          </fieldset>
        )}
        {claim && mode === "supersede" ? (
          <>
            {numeric ? (
              <>
                <label className="stack">
                  {t("whatif.valueNum")}
                  <input inputMode="decimal" value={num} onChange={(e) => setNum(e.target.value)} required />
                </label>
                <label className="stack">
                  {t("whatif.unit")}
                  <input value={unit} onChange={(e) => setUnit(e.target.value)} />
                </label>
              </>
            ) : (
              <label className="stack">
                {t("whatif.valueText")}
                <input value={text} onChange={(e) => setText(e.target.value)} required />
              </label>
            )}
            <label className="stack">
              {t("whatif.revision")}
              <input value={rev} onChange={(e) => setRev(e.target.value)} />
            </label>
            <label className="stack wide">
              {t("whatif.reason")}
              <input value={reason} maxLength={500} onChange={(e) => setReason(e.target.value)} />
            </label>
          </>
        ) : (
          <p className="muted wide">{claim ? t("whatif.withdrawNote") : t("whatif.withdrawDocNote", { doc: doc?.name ?? "" })}</p>
        )}
        <label className="wide">
          <input type="checkbox" checked={record} onChange={(e) => setRecord(e.target.checked)} /> {t("whatif.record")}
        </label>
        <div className="actions wide">
          <button className="primary" type="submit" disabled={busy}>
            {busy ? t("whatif.running") : t("whatif.run")}
          </button>
        </div>
      </form>
      <Notice error={err} />
      <div aria-live="polite">{res && <SimView r={res} why={why} />}</div>
      {res && (
        <p className="muted">
          {t("whatif.readOnly")}
          {res.run_id && ` ${t("whatif.recorded", { id: short(res.run_id) })}`}
        </p>
      )}
    </section>
  );
}

function ClaimCard({ c }: { c: Claim }) {
  const { t } = useI18n();
  return (
    <dl className="kv">
      <dt>{t("claim.id")}</dt>
      <dd className="mono">{c.id ? short(c.id) : "—"}</dd>
      <dt>{c.predicate}</dt>
      <dd className="mono">
        <b>{claimValue(c)}</b>
      </dd>
      <dt>{t("whatif.revision")}</dt>
      <dd className="mono">{c.revision ?? "—"}</dd>
    </dl>
  );
}

function DiffBlock({ title, d, label }: { title: string; d: SimDiff; label: (v: Record<string, unknown>) => string }) {
  const { t } = useI18n();
  const none = d.appear_total + d.clear_total + d.changed_total === 0;
  return (
    <section aria-label={title}>
      <h4>{title}</h4>
      {none ? (
        <p className="muted">{t("whatif.nothing")}</p>
      ) : (
        <>
          {d.appear_total > 0 && (
            <>
              <Badge group="whatif" value={`${t("whatif.appear")} ${d.appear_total}`} tone="crit" />
              <ul>
                {d.appear.slice(0, 20).map((v, i) => (
                  <li key={i}>{label(v)}</li>
                ))}
              </ul>
            </>
          )}
          {d.clear_total > 0 && (
            <>
              <Badge group="whatif" value={`${t("whatif.clear")} ${d.clear_total}`} tone="ok" />
              <ul>
                {d.clear.slice(0, 20).map((v, i) => (
                  <li key={i}>{label(v)}</li>
                ))}
              </ul>
            </>
          )}
          {d.changed_total > 0 && <Badge group="whatif" value={`${t("whatif.changed")} ${d.changed_total}`} tone="warn" />}
        </>
      )}
    </section>
  );
}

export function SimView({ r, why }: { r: SimResult; why: (v: Record<string, unknown>) => string }) {
  const { t, tv, fmtNum } = useI18n();
  const d = r.decisions;
  const before = r.change.before;
  return (
    <>
      <div className="whatif-sbs">
        <section aria-label={t("whatif.before")}>
          <h4>{t("whatif.before")}</h4>
          {before.slice(0, 3).map((c) => (
            <ClaimCard key={c.id} c={c} />
          ))}
          {r.change.before_total > 3 && <p className="muted">{t("whatif.moreClaims", { n: r.change.before_total - 3 })}</p>}
        </section>
        <section className="after" aria-label={t("whatif.after")}>
          <h4>{t("whatif.after")}</h4>
          {r.change.after ? <ClaimCard c={r.change.after} /> : <p className="callout warn">{t("whatif.withdrawn")}</p>}
        </section>
      </div>
      <h4>{t("whatif.decisions")}</h4>
      <ul className="whatif-stats">
        <li>
          <b>{fmtNum(d.affected_total)}</b>
          {t("whatif.affected")}
        </li>
        <li>
          <b>{fmtNum(d.material_total)}</b>
          {t("whatif.material")}
        </li>
        <li>
          <b>{fmtNum(d.newly_flagged_total)}</b>
          {t("whatif.newlyFlagged")}
        </li>
      </ul>
      {d.bounded && <p className="callout warn">{t("whatif.bounded", { limit: fmtNum(d.limit) })}</p>}
      {d.review_batch && !d.bounded && <p className="callout warn">{t("whatif.reviewBatch")}</p>}
      {d.truncated && <p className="muted">{t("whatif.truncated", { shown: d.sample.length, total: fmtNum(d.affected_total) })}</p>}
      {d.affected_total === 0 && !d.bounded ? (
        <p className="muted">{t("whatif.noDecisions")}</p>
      ) : (
        <>
          <div className="table-wrap" tabIndex={0} role="region" aria-label={t("whatif.decisions")}>
            <table>
              <thead>
                <tr>
                  <th scope="col">{t("whatif.decisions")}</th>
                  <th scope="col">{t("whatif.statusChange")}</th>
                  <th scope="col">{t("whatif.material")}</th>
                  <th scope="col">{t("whatif.reasonCol")}</th>
                  <th scope="col">{t("whatif.path")}</th>
                </tr>
              </thead>
              <tbody>
                {d.sample.map((i) => (
                  <tr key={i.decision_id} className={i.material ? "row-crit" : undefined}>
                    <td>
                      {i.title ?? short(i.decision_id)} <span className="mono muted">{short(i.decision_id)}</span>
                    </td>
                    <td>
                      <Badge group="decision.status" value={i.status_before} /> → <Badge group="decision.status" value={i.status_after} />
                    </td>
                    <td>
                      <Badge group="bool" value={i.material ? "material" : "not_material"} tone={i.material ? "crit" : "info"} />
                    </td>
                    <td>
                      <Badge group="cause" value={i.cause_kind} tone="info" /> {i.reason}
                    </td>
                    <td className="mono">{i.path.map(short).join(" → ")}</td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
          {r.graph.nodes.length > 0 && (
            <>
              <h4>{t("whatif.graph")}</h4>
              <GraphScene g={r.graph} label={t("whatif.graphLabel", { n: d.sample.length })} onPick={() => {}} height={260} />
            </>
          )}
        </>
      )}
      <div className="whatif-diff">
        <DiffBlock
          title={t("whatif.matching")}
          d={r.matching}
          label={(v) => `${v.purchase_order}/${v.po_line} ${v.part}: ${tv("match.kind", String(v.kind))}`}
        />
        <DiffBlock title={t("whatif.signals")} d={r.integrity.signals} label={(v) => `${v.supplier ?? ""} — ${why(v)}`} />
        <DiffBlock title={t("whatif.drift")} d={r.integrity.drift} label={(v) => `${v.key}: ${why(v)}`} />
      </div>
      <h4>{t("whatif.resupply")}</h4>
      {!r.resupply || r.resupply.suppliers.length === 0 ? (
        <p className="muted">{t("whatif.resupplyNone")}</p>
      ) : (
        <>
          <ul>
            {r.resupply.suppliers.map((s) => (
              <li key={s.supplier}>
                {t("whatif.resupplyLine", { supplier: s.supplier, n: s.documents.length, baseline: s.baseline_count })}:{" "}
                {s.documents.map((x) => x.name || short(x.doc)).join(", ")}
              </li>
            ))}
          </ul>
          <p className="muted">{t("whatif.resupplyNote")}</p>
        </>
      )}
    </>
  );
}
