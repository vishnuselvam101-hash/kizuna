import { useMemo, useState } from "react";
import { ClaimId, ClaimPicker } from "./claims";
import { DataTable } from "./DataTable";
import { snapValue } from "./format";
import { GraphScene, type Graph } from "./graph";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Decision, Trace } from "./types";
import { Badge, download, Empty, Loading, Notice, PageTitle, Panel, short, useLoad } from "./ui";
import { simPreview, useSimPreview } from "./WhatIfPanel";

export function traceGraph(t: Trace): Graph {
  const d = t.decision;
  const nodes: Graph["nodes"] = [{ id: d.id, kind: "decision", label: d.title, tone: d.status === "valid" ? "ok" : "crit" }];
  const edges: Graph["edges"] = [];
  t.evidence.forEach((e, i) => {
    const id = e.claim_id ?? e.upstream_decision ?? `e${i}`;
    nodes.push({
      id,
      kind: e.upstream_decision ? "subject" : "claim",
      label: `${e.predicate ?? short(e.upstream_decision ?? "")} = ${snapValue(e.snapshot_at_decision)}`,
      tone: e.changed ? "crit" : "info",
    });
    edges.push([id, d.id]);
  });
  t.changed_since_decision.forEach((c) => {
    const id = `chg-${c.claim_id}`;
    if (nodes.some((n) => n.id === id)) return;
    nodes.push({ id, kind: "document", label: `${c.predicate} → ${snapValue(c.value)}`, tone: "warn" });
    const ev = t.evidence.find((e) => e.predicate === c.predicate);
    edges.push([id, ev?.claim_id ?? d.id]);
  });
  return { nodes, edges };
}

export function TraceView({ t, onAction, canAct }: { t: Trace; onAction: (a: "reconfirm" | "revoke", comment: string) => void; canAct: boolean }) {
  const { t: tr, tv, fmtDate } = useI18n();
  const [comment, setComment] = useState("");
  const [confirmRevoke, setConfirmRevoke] = useState(false);
  const d = t.decision;
  const g = useMemo(() => traceGraph(t), [t]);
  const open = d.status === "valid" || d.status === "potentially_affected";
  // An active what-if preview highlights this decision and the evidence it would change.
  const { sim, ids } = useSimPreview();
  const hl = useMemo(() => new Set(g.nodes.filter((n) => ids.has(n.id)).map((n) => n.id)), [g, ids]);
  const simHit = sim?.decisions.sample.find((i) => i.decision_id === d.id);
  return (
    <article className="trace" aria-labelledby="trace-h">
      <header className="finding-head">
        <h2 id="trace-h">{d.title}</h2>
        <Badge group="decision.status" value={d.status} />
      </header>
      <dl className="kv">
        <dt>{tr("decisions.kind")}</dt>
        <dd>{tv("decision.kind", d.kind)}</dd>
        <dt>{tr("decisions.outcome")}</dt>
        <dd>{tv("decision.outcome", d.outcome)}</dd>
        <dt>{tr("decisions.decidedAt")}</dt>
        <dd className="num">{fmtDate(d.decided_at)}</dd>
        <dt>{tr("decisions.derivation")}</dt>
        <dd>
          <code title={d.derivation_sha256}>{d.derivation_sha256.slice(0, 16)}…</code>{" "}
          <Badge group="bool" value={t.derivation_reproducible ? "reproducible" : "not_reproducible"} tone={t.derivation_reproducible ? "ok" : "crit"} />
        </dd>
        <dt>{tr("decisions.rationale")}</dt>
        <dd>{d.rationale}</dd>
      </dl>
      <h3>{tr("trace.graph")}</h3>
      {sim && (
        <p className="callout warn" role="status">
          {simHit ? tr("whatif.traceAffected", { status: tv("decision.status", simHit.status_after) }) : tr("whatif.highlight")}{" "}
          <button type="button" className="link" onClick={() => simPreview.set(null)}>
            {tr("whatif.clearHighlight")}
          </button>
        </p>
      )}
      <GraphScene g={g} label={tr("trace.graphLabel", { n: t.evidence.length })} onPick={() => {}} height={280} highlight={hl.size ? hl : undefined} />
      <h3>{tr("trace.evidence")}</h3>
      <div className="table-wrap">
        <table>
          <thead>
            <tr>
              <th scope="col">{tr("trace.item")}</th>
              <th scope="col">{tr("trace.atDecision")}</th>
              <th scope="col">{tr("trace.now")}</th>
              <th scope="col">{tr("trace.changed")}</th>
              <th scope="col">{tr("claim.id")}</th>
            </tr>
          </thead>
          <tbody>
            {t.evidence.map((e, i) => (
              <tr key={i} className={e.changed ? "row-crit" : undefined}>
                <td className="mono">{e.predicate ?? tr("trace.upstream", { id: short(e.upstream_decision ?? "") })}</td>
                <td className="mono">{snapValue(e.snapshot_at_decision)}</td>
                <td className="mono">{e.current_value ? snapValue(e.current_value) : e.current_status && tv("decision.status", e.current_status)}</td>
                <td>{e.changed ? <span className="badge crit">{tr("common.yes")}</span> : tr("common.no")}</td>
                <td>{e.claim_id ? <ClaimId id={e.claim_id} /> : "—"}</td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <h3>{tr("trace.invalidations")}</h3>
      {t.invalidations.length === 0 ? (
        <p className="muted">{tr("trace.noInvalidations")}</p>
      ) : (
        <ol className="timeline">
          {t.invalidations.map((i, k) => (
            <li key={k}>
              <span className="num muted">{fmtDate(i.recorded_at)}</span> <Badge group="cause" value={i.cause_kind} tone="info" /> {i.reason}{" "}
              <Badge group="bool" value={i.material ? "material" : "not_material"} tone={i.material ? "crit" : "info"} />
            </li>
          ))}
        </ol>
      )}
      <EvidencePack t={t} />
      {d.status === "potentially_affected" && <p className="callout crit">{tr("trace.mustReview")}</p>}
      {open && canAct && (
        <section className="review-box" aria-labelledby="trace-act">
          <h3 id="trace-act">{tr("trace.act")}</h3>
          <label className="stack">
            {tr("finding.comment")}
            <textarea value={comment} onChange={(e) => setComment(e.target.value)} />
          </label>
          <div className="actions">
            {d.status === "potentially_affected" && (
              <button className="primary" onClick={() => onAction("reconfirm", comment)}>
                {tr("trace.reconfirm")}
              </button>
            )}
            {confirmRevoke ? (
              <span role="group" aria-label={tr("trace.confirmRevoke")} className="confirm">
                <span>{tr("trace.confirmRevoke")}</span>
                <button className="danger" onClick={() => (setConfirmRevoke(false), onAction("revoke", comment))}>
                  {tr("trace.revokeConfirm")}
                </button>
                <button onClick={() => setConfirmRevoke(false)}>{tr("common.cancel")}</button>
              </span>
            ) : (
              <button className="danger" onClick={() => setConfirmRevoke(true)}>
                {tr("trace.revoke")}
              </button>
            )}
          </div>
        </section>
      )}
    </article>
  );
}

/** Offline-verifiable evidence pack (ZIP) of the decision; selected documents can be exported as hash only. */
function EvidencePack({ t }: { t: Trace }) {
  const { api } = useSession();
  const { t: tr } = useI18n();
  const docs = useMemo(() => {
    const m = new Map<string, string[]>();
    for (const e of t.derivation?.evidence ?? []) {
      if (!e.artifact_id) continue;
      const preds = m.get(e.artifact_id) ?? [];
      if (e.predicate && !preds.includes(e.predicate)) preds.push(e.predicate);
      m.set(e.artifact_id, preds);
    }
    return [...m.entries()];
  }, [t]);
  const [hashOnly, setHashOnly] = useState<string[]>([]);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  async function exportPack() {
    setBusy(true);
    setErr(null);
    setMsg("");
    try {
      const zip = await api.blob("POST", `/decisions/${t.decision.id}/evidence-pack`, { hash_only: hashOnly });
      download(`kizuna-evidence-pack-${short(t.decision.id)}.zip`, zip, "application/zip");
      setMsg(tr("pack.done"));
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }
  return (
    <section className="review-box" aria-labelledby="pack-h">
      <h3 id="pack-h">{tr("pack.title")}</h3>
      <p className="muted">{tr("pack.lead")}</p>
      {docs.length > 0 && (
        <fieldset className="faulty">
          <legend>{tr("pack.hashOnlyLegend")}</legend>
          {docs.map(([id, preds]) => (
            <label key={id} className="check">
              <input
                type="checkbox"
                checked={hashOnly.includes(id)}
                onChange={(e) => setHashOnly((h) => (e.target.checked ? [...h, id] : h.filter((x) => x !== id)))}
              />
              {tr("pack.hashOnly", { doc: short(id), preds: preds.join(", ") || "—" })}
            </label>
          ))}
        </fieldset>
      )}
      <p className="hint">{tr("pack.notProven")}</p>
      <div className="actions">
        <button type="button" onClick={exportPack} disabled={busy} aria-busy={busy}>
          {busy ? tr("pack.exporting") : tr("pack.export")}
        </button>
      </div>
      <Notice msg={msg} error={err} />
    </section>
  );
}

/** Shared list → trace workspace used by Decisions and affected-decision analysis. */
export function DecisionWorkspace({ status, title, lead, children, reloadToken = 0 }: { status?: string; title: string; lead: string; children?: React.ReactNode; reloadToken?: number }) {
  const { api, can } = useSession();
  const { t, fmtDate } = useI18n();
  const list = useLoad(() => api.request<Decision[]>("GET", status ? `/decisions?status=${status}` : "/decisions"), [status, reloadToken]);
  const [trace, setTrace] = useState<Trace | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [msg, setMsg] = useState("");
  async function open(id: string) {
    setErr(null);
    try {
      setTrace(await api.request<Trace>("GET", `/decisions/${id}/trace`));
    } catch (x) {
      setErr(x);
    }
  }
  async function act(action: "reconfirm" | "revoke", comment: string) {
    if (!trace) return;
    setErr(null);
    try {
      const d = await api.request<Decision>("POST", `/decisions/${trace.decision.id}/${action}`, { comment });
      setMsg(t(action === "revoke" ? "trace.revoked" : "trace.reconfirmed"));
      await open(d.id);
      list.reload();
    } catch (x) {
      setErr(x);
    }
  }
  return (
    <div>
      <PageTitle title={title} lead={lead} />
      {children}
      <Notice msg={msg} error={err ?? list.error} />
      <div className="grid2 wide-right">
        <Panel title={t("decisions.list")} id="dec-list">
          <Loading on={list.loading} />
          {list.data && list.data.length === 0 && <Empty text={status ? t("affected.none") : t("decisions.none")} />}
          {list.data && list.data.length > 0 && (
            <DataTable
              label={t("decisions.list")}
              rows={list.data}
              rowKey={(d) => d.id}
              selectedKey={trace?.decision.id}
              onActivate={(d) => open(d.id)}
              cols={[
                {
                  key: "title",
                  label: t("decisions.title"),
                  value: (d) => d.title,
                  className: "col-title",
                  render: (d) => (
                    <button className="link" onClick={(e) => (e.stopPropagation(), open(d.id))}>
                      {d.title}
                    </button>
                  ),
                },
                { key: "subject", label: t("compare.part"), value: (d) => d.subject_key, render: (d) => <span className="mono">{d.subject_key}</span> },
                { key: "status", label: t("docs.status"), value: (d) => d.status, render: (d) => <Badge group="decision.status" value={d.status} /> },
                { key: "at", label: t("decisions.decidedAt"), value: (d) => d.decided_at, render: (d) => fmtDate(d.decided_at), className: "num" },
              ]}
            />
          )}
        </Panel>
        <div>{trace ? <TraceView t={trace} onAction={act} canAct={can("decision_write")} /> : <Empty text={t("decisions.select")} />}</div>
      </div>
    </div>
  );
}

const KINDS = ["supplier_part_release", "first_article_approval", "deviation_acceptance", "other"];

function RecordDecision({ onDone }: { onDone: () => void }) {
  const { api } = useSession();
  const { t, tv } = useI18n();
  const [f, setF] = useState({ title: "", kind: KINDS[0], subject_key: "", outcome: "approved", rationale: "" });
  const [claims, setClaims] = useState<string[]>([]);
  const [err, setErr] = useState<unknown>(null);
  const [msg, setMsg] = useState("");
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    try {
      await api.request("POST", "/decisions", { ...f, subject_type: "part", evidence: claims.map((claim_id) => ({ claim_id })) });
      setMsg(t("decisions.recorded"));
      setClaims([]);
      onDone();
    } catch (x) {
      setErr(x);
    }
  }
  const input = (k: keyof typeof f, label: string, hint?: string) => (
    <label>
      {label}
      <input
        required
        value={f[k]}
        onChange={(e) => {
          setF({ ...f, [k]: e.target.value });
          // Evidence belongs to one record: changing the record clears the picked claims.
          if (k === "subject_key") setClaims([]);
        }}
        aria-describedby={hint ? `${k}-hint` : undefined}
      />
      {hint && (
        <span id={`${k}-hint`} className="hint">
          {hint}
        </span>
      )}
    </label>
  );
  return (
    <details className="panel collapsible">
      <summary>{t("decisions.record")}</summary>
      <form onSubmit={submit} className="form-grid">
        {input("title", t("decisions.title"))}
        {input("subject_key", t("compare.part"))}
        <label>
          {t("decisions.kind")}
          <select value={f.kind} onChange={(e) => setF({ ...f, kind: e.target.value })}>
            {KINDS.map((k) => (
              <option key={k} value={k}>
                {tv("decision.kind", k)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("decisions.outcome")}
          <select value={f.outcome} onChange={(e) => setF({ ...f, outcome: e.target.value })}>
            {["approved", "rejected", "conditional"].map((o) => (
              <option key={o} value={o}>
                {tv("decision.outcome", o)}
              </option>
            ))}
          </select>
        </label>
        {input("rationale", t("decisions.rationale"))}
        <div className="span-all">
          <ClaimPicker subjectType="part" subjectKey={f.subject_key} selected={claims} onChange={setClaims} />
          <p className="hint">{t("decisions.evidenceHint")}</p>
        </div>
        <div>
          <button className="primary">{t("decisions.recordBtn")}</button>
        </div>
      </form>
      <Notice msg={msg} error={err} />
    </details>
  );
}

export function DecisionsView() {
  const { can } = useSession();
  const { t } = useI18n();
  const [key, setKey] = useState(0);
  return (
    <DecisionWorkspace reloadToken={key} title={t("nav.decisions")} lead={t("decisions.lead")}>
      {can("decision_write") && <RecordDecision onDone={() => setKey((k) => k + 1)} />}
    </DecisionWorkspace>
  );
}

export function AffectedView() {
  const { t } = useI18n();
  return <DecisionWorkspace status="potentially_affected" title={t("nav.affected")} lead={t("affected.lead")} />;
}
