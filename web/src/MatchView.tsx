import { useState } from "react";
import { DataTable } from "./DataTable";
import { sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, download, useLoad, type Tone } from "./ui";

/* ---------- API shapes (mfg-match@1, mfg-resupply@1) ---------- */
export interface LineSummary {
  purchase_order: string;
  po_line: string;
  part: string;
  supplier: string | null;
  ordered_revision: string | null;
  certificates: number;
  critical: number;
  major: number;
  minor: number;
  kinds: string[];
  acceptance: { decision_id: string; outcome: string; status: string; decided_at: string } | null;
}
interface Cell {
  value: string | null;
  claims: string[];
}
interface CheckRow {
  check: string;
  status: string;
  po: Cell;
  drawing: Cell;
  certificate: Cell;
}
export interface MatchFinding {
  kind: string;
  severity: "critical" | "major" | "minor";
  check: string;
  po_claims: string[];
  drawing_claims: string[];
  certificate_claims: string[];
  certificate_doc: string | null;
  explanation: { rule: string; why: string; msg?: unknown; detail?: Record<string, unknown> };
}
interface CitedClaim {
  predicate: string;
  value_text: string | null;
  value_num: string | null;
  value_unit: string | null;
  revision: string | null;
  artifact_id: string | null;
  source_location: Record<string, string | number>;
}
export interface MatchDetail {
  match: {
    rule_version: string;
    line: { purchase_order: string; line: string; part: string };
    supplier: string | null;
    ordered_revision: string | null;
    drawing_revision: string | null;
    latest_drawing_revision: string | null;
    certificates: { doc: string; linked_by: string; lots: string[]; heats: string[] }[];
    checks: CheckRow[];
    findings: MatchFinding[];
  };
  blocking: number;
  citations: { claims: Record<string, CitedClaim>; documents: Record<string, { kind: string; display_name: string; sha256: string }> };
  runs: { id: string; created_at: string; blocking: number; decision_id: string | null; decision_status: string | null; outcome: string | null }[];
}

const STATUS_TONE: Record<string, Tone> = { match: "ok", mismatch: "crit", missing: "warn", not_required: "neutral", not_evaluated: "neutral" };
const SEV_TONE: Record<string, Tone> = { critical: "crit", major: "warn", minor: "info" };
const SIDES = ["po", "drawing", "certificate"] as const;

function checkLabel(tv: (g: string, v: string) => string, check: string) {
  const [head, rest] = check.split(/\.(.+)/);
  return rest ? `${tv("match.check", head)}: ${tv("match.attr", rest)}` : tv("match.check", check);
}

function Citation({ id, d }: { id: string; d: MatchDetail }) {
  const { t } = useI18n();
  const c = d.citations.claims[id];
  if (!c) return <li className="mono">{id.slice(0, 8)}</li>;
  const doc = c.artifact_id ? d.citations.documents[c.artifact_id] : undefined;
  const v = c.value_num ?? c.value_text ?? "";
  return (
    <li>
      <span className="mono">{c.predicate}</span> = <b className="mono">{c.value_unit ? `${v} ${c.value_unit}` : v}</b>
      <br />
      <span className="muted">
        {doc ? doc.display_name : t("evidence.manual")} · {sourceLocation(t, c.source_location)}
        {doc && (
          <>
            {" · "}
            <code title={doc.sha256}>{doc.sha256.slice(0, 12)}…</code>
          </>
        )}
      </span>
    </li>
  );
}

function FindingCard({ f, d }: { f: MatchFinding; d: MatchDetail }) {
  const { tm, t, tv } = useI18n();
  const ids = { po: f.po_claims, drawing: f.drawing_claims, certificate: f.certificate_claims };
  return (
    <li className="finding">
      <header className="finding-head">
        <Badge group="match.sev" value={f.severity} tone={SEV_TONE[f.severity]} /> <b>{tv("match.kind", f.kind)}</b>{" "}
        <span className="muted">· {checkLabel(tv, f.check)}</span>
      </header>
      <p>{tm(f.explanation.msg, f.explanation.why)}</p>
      <div className="match-sides">
        {SIDES.map((s) => (
          <section key={s} aria-label={t(`match.side.${s}` as never)}>
            <h4>{t(`match.side.${s}` as never)}</h4>
            {ids[s].length === 0 ? <p className="muted">{t("common.none")}</p> : <ul className="list">{ids[s].map((id) => <Citation key={id} id={id} d={d} />)}</ul>}
          </section>
        ))}
      </div>
    </li>
  );
}

export function MatchDetailView({ d, canAccept, onAccept }: { d: MatchDetail; canAccept: boolean; onAccept: (outcome: string, rationale: string) => void }) {
  const { t, tv, fmtDate } = useI18n();
  const [outcome, setOutcome] = useState(d.blocking > 0 ? "conditional" : "approved");
  const [rationale, setRationale] = useState("");
  const [need, setNeed] = useState(false);
  const m = d.match;
  return (
    <article aria-labelledby="match-h">
      <header className="finding-head">
        <h2 id="match-h">
          {m.line.purchase_order} / {m.line.line} · <span className="mono">{m.line.part}</span>
        </h2>
        <Badge group="match.blocking" value={d.blocking > 0 ? "blocked" : "clear"} tone={d.blocking > 0 ? "crit" : "ok"} />
      </header>
      <dl className="kv">
        <dt>{t("match.supplier")}</dt>
        <dd>{m.supplier ?? "—"}</dd>
        <dt>{t("match.revisions")}</dt>
        <dd className="mono">
          {t("match.revLine", { ordered: m.ordered_revision ?? "—", drawing: m.drawing_revision ?? "—", latest: m.latest_drawing_revision ?? "—" })}
        </dd>
        <dt>{t("match.certs")}</dt>
        <dd>
          {m.certificates.length === 0
            ? t("common.none")
            : m.certificates.map((c) => `${d.citations.documents[c.doc]?.display_name ?? c.doc.slice(0, 8)} (${[...c.lots.map((l) => `lot ${l}`), ...c.heats.map((h) => `heat ${h}`)].join(", ") || "—"})`).join("; ")}
        </dd>
        <dt>{t("match.rule")}</dt>
        <dd className="mono">{m.rule_version}</dd>
      </dl>
      <div className="table-wrap" tabIndex={0} role="region" aria-label={t("match.matrix")}>
        <table className="match-matrix">
          <caption className="sr-only">{t("match.matrix")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("match.check")}</th>
              <th scope="col">{t("match.side.po")}</th>
              <th scope="col">{t("match.side.drawing")}</th>
              <th scope="col">{t("match.side.certificate")}</th>
              <th scope="col">{t("match.status")}</th>
            </tr>
          </thead>
          <tbody>
            {m.checks.map((r) => (
              <tr key={r.check} className={r.status === "mismatch" ? "row-crit" : undefined}>
                <th scope="row">{checkLabel(tv, r.check)}</th>
                {SIDES.map((s) => (
                  <td key={s} className="mono">
                    {r[s].value ?? "—"}
                  </td>
                ))}
                <td>
                  <Badge group="match.status" value={r.status} tone={STATUS_TONE[r.status]} />
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>
      <h3>{t("match.findings", { n: m.findings.length })}</h3>
      {m.findings.length === 0 ? <p className="muted">{t("match.noFindings")}</p> : <ol className="plain">{m.findings.map((f, i) => <FindingCard key={i} f={f} d={d} />)}</ol>}
      <h3>{t("match.runs")}</h3>
      {d.runs.length === 0 ? (
        <p className="muted">{t("match.noRuns")}</p>
      ) : (
        <ol className="timeline">
          {d.runs.map((r) => (
            <li key={r.id}>
              <span className="num muted">{fmtDate(r.created_at)}</span> {t("match.runBlocking", { n: r.blocking })}{" "}
              {r.decision_id && r.outcome && <Badge group="decision.outcome" value={r.outcome} />} {r.decision_status && <Badge group="decision.status" value={r.decision_status} />}
            </li>
          ))}
        </ol>
      )}
      {canAccept && (
        <section className="review-box" aria-labelledby="match-acc">
          <h3 id="match-acc">{t("match.accept")}</h3>
          <label className="stack">
            {t("decisions.outcome")}
            <select value={outcome} onChange={(e) => setOutcome(e.target.value)}>
              {["approved", "conditional", "rejected"].map((o) => (
                <option key={o} value={o} disabled={o === "approved" && d.blocking > 0}>
                  {tv("decision.outcome", o)}
                </option>
              ))}
            </select>
          </label>
          {d.blocking > 0 && <p className="callout crit">{t("match.blockedHint", { n: d.blocking })}</p>}
          <label className="stack">
            {t("decisions.rationale")}
            <textarea value={rationale} onChange={(e) => setRationale(e.target.value)} aria-invalid={need} aria-describedby="match-acc-hint" />
          </label>
          <p id="match-acc-hint" className={need ? "notice crit" : "muted"}>
            {t("match.acceptHint")}
          </p>
          <div className="actions">
            <button
              className="primary"
              onClick={() => {
                if (!rationale.trim()) return setNeed(true);
                setNeed(false);
                onAccept(outcome, rationale.trim());
              }}
            >
              {t("match.acceptBtn")}
            </button>
          </div>
        </section>
      )}
    </article>
  );
}

const lineKey = (l: LineSummary) => `${l.purchase_order}/${l.po_line}/${l.part}`;

export function MatchView() {
  const { api, can } = useSession();
  const { t, tv } = useI18n();
  const list = useLoad(() => api.request<{ lines: LineSummary[] }>("GET", "/match/lines"), []);
  const [sel, setSel] = useState<LineSummary | null>(null);
  const [detail, setDetail] = useState<MatchDetail | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const q = (l: LineSummary) => `po=${encodeURIComponent(l.purchase_order)}&line=${encodeURIComponent(l.po_line)}&part=${encodeURIComponent(l.part)}`;
  async function open(l: LineSummary) {
    setErr(null);
    setSel(l);
    try {
      setDetail(await api.request<MatchDetail>("GET", `/match/line?${q(l)}`));
    } catch (x) {
      setErr(x);
    }
  }
  async function accept(outcome: string, rationale: string) {
    if (!sel) return;
    setErr(null);
    setMsg("");
    try {
      await api.request("POST", "/match/line/accept", { po: sel.purchase_order, line: sel.po_line, part: sel.part, outcome, rationale });
      setMsg(t("match.accepted"));
      await open(sel);
      list.reload();
    } catch (x) {
      setErr(x);
    }
  }
  const rows = list.data?.lines ?? [];
  return (
    <div>
      <PageTitle title={t("nav.match")} lead={t("match.lead")} />
      <Notice msg={msg} error={err ?? list.error} />
      <div className="grid2 wide-right">
        <Panel title={t("match.lines")} id="m-list">
          <Loading on={list.loading} />
          {list.data && rows.length === 0 && <Empty text={t("match.none")} />}
          {rows.length > 0 && (
            <DataTable
              label={t("match.lines")}
              rows={rows}
              rowKey={lineKey}
              selectedKey={sel ? lineKey(sel) : null}
              onActivate={open}
              rowClass={(l) => (l.critical > 0 ? "row-crit" : undefined)}
              cols={[
                { key: "po", label: t("match.poLine"), value: (l) => `${l.purchase_order}/${l.po_line}`, render: (l) => <span className="mono">{`${l.purchase_order} / ${l.po_line}`}</span> },
                { key: "part", label: t("compare.part"), value: (l) => l.part, render: (l) => <span className="mono">{l.part}</span> },
                { key: "supplier", label: t("match.supplier"), value: (l) => l.supplier ?? "", render: (l) => l.supplier ?? "—" },
                {
                  key: "result",
                  label: t("match.result"),
                  value: (l) => l.critical * 1000 + l.major * 10 + l.minor,
                  render: (l) =>
                    l.critical + l.major + l.minor === 0 ? (
                      <Badge group="match.status" value="match" tone="ok" />
                    ) : (
                      <span title={l.kinds.map((k) => tv("match.kind", k)).join(", ")}>
                        {l.critical > 0 && <span className="badge crit">{t("match.nCritical", { n: l.critical })}</span>}{" "}
                        {l.major > 0 && <span className="badge warn">{t("match.nMajor", { n: l.major })}</span>}{" "}
                        {l.minor > 0 && <span className="badge info">{t("match.nMinor", { n: l.minor })}</span>}
                      </span>
                    ),
                },
                {
                  key: "acc",
                  label: t("match.acceptance"),
                  value: (l) => l.acceptance?.status ?? "",
                  render: (l) => (l.acceptance ? <Badge group="decision.status" value={l.acceptance.status} /> : <span className="muted">—</span>),
                },
              ]}
            />
          )}
        </Panel>
        <div>{detail ? <MatchDetailView key={lineKey(sel!)} d={detail} canAccept={can("decision_write")} onAccept={accept} /> : <Empty text={t("match.select")} />}</div>
      </div>
    </div>
  );
}

/* ---------- Minimal supplier re-requests ---------- */
interface ReqDoc {
  doc: string;
  name: string;
  sha256: string;
  items: { part: string; predicate: string; purchase_order: string | null; po_line: string | null; lot: string | null; heat: string | null }[];
  reasons: { predicate: string; from: string | null; to: string | null; cause: string }[];
}
export interface Plan {
  engine: string;
  decisions_considered: number;
  suppliers: { supplier: string; documents: ReqDoc[]; baseline_count: number; exact: boolean; uncovered_items: string[]; decision_ids: string[] }[];
  no_supplier_action: { decision_id: string; changed_predicates: string[] }[];
}
export interface EvidenceRequest {
  id: string;
  reference: string;
  supplier: string;
  status: string;
  requested_count: number;
  baseline_count: number;
  exact: boolean;
  body_ja: string;
  body_en: string;
  content_sha256: string;
  created_by: string;
  created_at: string;
  review_comment: string | null;
}

function RequestDetail({ r, onReview, canReview }: { r: EvidenceRequest; onReview: (to: "approve" | "cancel", comment: string) => void; canReview: boolean }) {
  const { api } = useSession();
  const { t, tv, lang } = useI18n();
  const [l, setL] = useState<"ja" | "en">(lang);
  const [comment, setComment] = useState("");
  const [err, setErr] = useState<unknown>(null);
  async function save() {
    setErr(null);
    try {
      download(`${r.reference}-${l}.txt`, await api.text(`/evidence-requests/${r.id}/document?lang=${l}`), "text/plain;charset=utf-8");
    } catch (x) {
      setErr(x);
    }
  }
  return (
    <article aria-labelledby="rr-h" className="rr-detail">
      <header className="finding-head">
        <h2 id="rr-h">
          {r.reference} · {r.supplier}
        </h2>
        <Badge group="rr.status" value={r.status} tone={r.status === "approved" ? "ok" : r.status === "draft" ? "warn" : "neutral"} />
      </header>
      <p>{t("rr.reduction", { n: r.requested_count, of: r.baseline_count })}</p>
      <div className="toolbar no-print" role="group" aria-label={t("rr.language")}>
        {(["ja", "en"] as const).map((x) => (
          <button key={x} aria-pressed={l === x} onClick={() => setL(x)}>
            {tv("rr.lang", x)}
          </button>
        ))}
        <button onClick={save}>{t("rr.download")}</button>
        <button onClick={() => window.print()}>{t("rr.print")}</button>
      </div>
      <Notice error={err} />
      {r.status !== "approved" && <p className="callout warn">{t("rr.draftNote")}</p>}
      <section aria-label={t("rr.letter")}>
        <pre className="letter" lang={l} tabIndex={0}>
          {l === "ja" ? r.body_ja : r.body_en}
        </pre>
      </section>
      <p className="muted">
        SHA-256 <code>{r.content_sha256.slice(0, 16)}…</code>
      </p>
      {canReview && r.status === "draft" && (
        <section className="review-box no-print" aria-labelledby="rr-rev">
          <h3 id="rr-rev">{t("rr.review")}</h3>
          <label className="stack">
            {t("finding.comment")}
            <textarea value={comment} onChange={(e) => setComment(e.target.value)} />
          </label>
          <p className="muted">{t("rr.reviewHint")}</p>
          <div className="actions">
            <button className="primary" onClick={() => onReview("approve", comment)}>
              {t("rr.approve")}
            </button>
            <button className="danger" onClick={() => onReview("cancel", comment)}>
              {t("rr.cancel")}
            </button>
          </div>
        </section>
      )}
    </article>
  );
}

export function ReRequestView() {
  const { api, can } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const plan = useLoad(() => api.request<Plan>("GET", "/evidence-requests/preview"), []);
  const reqs = useLoad(() => api.request<EvidenceRequest[]>("GET", "/evidence-requests"), []);
  const [sel, setSel] = useState<EvidenceRequest | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  async function guard(fn: () => Promise<void>) {
    setErr(null);
    setMsg("");
    try {
      await fn();
    } catch (x) {
      setErr(x);
    }
  }
  const create = () =>
    guard(async () => {
      const r = await api.request<EvidenceRequest[]>("POST", "/evidence-requests", {});
      setMsg(t("rr.created", { n: r.length }));
      reqs.reload();
      if (r[0]) setSel(r[0]);
    });
  const review = (to: "approve" | "cancel", comment: string) =>
    guard(async () => {
      if (!sel) return;
      setSel(await api.request<EvidenceRequest>("POST", `/evidence-requests/${sel.id}/${to}`, { comment: comment.trim() || null }));
      setMsg(t(to === "approve" ? "rr.approved" : "rr.cancelled"));
      reqs.reload();
    });
  const p = plan.data;
  const withDocs = p?.suppliers.filter((s) => s.documents.length > 0) ?? [];
  return (
    <div>
      <PageTitle title={t("nav.rerequests")} lead={t("rr.lead")} />
      <Notice msg={msg} error={err ?? plan.error ?? reqs.error} />
      <Panel
        title={t("rr.plan")}
        id="rr-plan"
        actions={
          can("decision_write") && withDocs.length > 0 ? (
            <button className="primary" onClick={create}>
              {t("rr.createDrafts")}
            </button>
          ) : undefined
        }
      >
        <Loading on={plan.loading} />
        {p && (
          <>
            <p className="muted">{t("rr.considered", { n: p.decisions_considered })}</p>
            {withDocs.length === 0 && <Empty text={t("rr.nothing")} />}
            {withDocs.map((s) => (
              <section key={s.supplier} className="rr-supplier" aria-label={s.supplier}>
                <h3>
                  {s.supplier} <span className="badge info">{t("rr.reduction", { n: s.documents.length, of: s.baseline_count })}</span>{" "}
                  {s.exact && <span className="badge ok">{t("rr.exact")}</span>}
                </h3>
                <ul className="list">
                  {s.documents.map((d) => (
                    <li key={d.doc}>
                      <b>{d.name}</b>{" "}
                      <span className="muted">
                        — {d.items.map((i) => `${i.part}${i.lot ? ` lot ${i.lot}` : ""}${i.heat ? ` heat ${i.heat}` : ""}: ${tv("rr.pred", i.predicate)}`).join("; ")}
                      </span>
                      <br />
                      <span className="muted">{d.reasons.map((r) => `${tv("rr.pred", r.predicate)}: ${r.from ?? "—"} → ${r.to ?? "—"}`).join("; ")}</span>
                    </li>
                  ))}
                </ul>
              </section>
            ))}
            {p.no_supplier_action.length > 0 && <p className="muted">{t("rr.internal", { n: p.no_supplier_action.length })}</p>}
          </>
        )}
      </Panel>
      <div className="grid2 wide-right">
        <Panel title={t("rr.drafts")} id="rr-list">
          <Loading on={reqs.loading} />
          {reqs.data && reqs.data.length === 0 && <Empty text={t("rr.none")} />}
          {reqs.data && reqs.data.length > 0 && (
            <DataTable
              label={t("rr.drafts")}
              rows={reqs.data}
              rowKey={(r) => r.id}
              selectedKey={sel?.id}
              onActivate={setSel}
              cols={[
                { key: "ref", label: t("rr.reference"), value: (r) => r.reference, render: (r) => <span className="mono">{r.reference}</span> },
                { key: "sup", label: t("match.supplier"), value: (r) => r.supplier, render: (r) => r.supplier },
                { key: "docs", label: t("rr.documents"), value: (r) => r.requested_count, render: (r) => <span className="num">{`${r.requested_count} / ${r.baseline_count}`}</span> },
                { key: "status", label: t("match.status"), value: (r) => r.status, render: (r) => <Badge group="rr.status" value={r.status} tone={r.status === "approved" ? "ok" : r.status === "draft" ? "warn" : "neutral"} /> },
                { key: "at", label: t("rr.created_at"), value: (r) => r.created_at, render: (r) => <span className="num">{fmtDate(r.created_at)}</span> },
              ]}
            />
          )}
        </Panel>
        <div>{sel ? <RequestDetail key={sel.id + sel.status} r={sel} onReview={review} canReview={can("review_decide")} /> : <Empty text={t("rr.select")} />}</div>
      </div>
    </div>
  );
}
