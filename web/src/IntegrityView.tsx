import { useState } from "react";
import { DataTable } from "./DataTable";
import { sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad, type Tone } from "./ui";

export interface EvidenceRef {
  artifact: string;
  document: string;
  claim: string | null;
  field: string | null;
  value: string | null;
  location: Record<string, string | number>;
}
export interface IntegritySignal {
  id: string;
  kind: string;
  supplier_key: string;
  part_key: string | null;
  characteristic: string | null;
  strength: string;
  explanation: { why: string; why_ja: string };
  statistic: Record<string, unknown>;
  evidence: EvidenceRef[];
  status: string;
  detected_at: string;
}
interface Review {
  decision: string;
  comment: string | null;
  decided_at: string;
}
interface Doc {
  artifact_id: string;
  display_name: string;
  sha256: string;
}
export interface SignalDetail {
  signal: IntegritySignal;
  documents: Doc[];
  reviews: Review[];
  requires_human_review: boolean;
  limitations: string;
}
interface Side {
  value: string;
  claim: string;
  location: Record<string, string | number>;
}
export interface DriftChange {
  predicate: string;
  category: string;
  change: string;
  before: Side[];
  after: Side[];
}
export interface DriftFinding {
  id: string;
  kind: string;
  drawing_key: string;
  part_key: string;
  earlier_artifact: string;
  later_artifact: string;
  earlier_revision: string | null;
  later_revision: string | null;
  changes: DriftChange[];
  strength: string;
  explanation: { why: string; why_ja: string };
  status: string;
  detected_at: string;
}
export interface DriftDetail {
  drift: DriftFinding;
  documents: Doc[];
  reviews: Review[];
  requires_human_review: boolean;
  potentially_affected_decisions: { id: string; title: string; status: string }[];
  limitations: string;
}

const SIGNAL_STATUSES = ["open", "needs_evidence", "explained", "follow_up", "obsolete"] as const;
const DRIFT_STATUSES = ["open", "needs_evidence", "accepted_change", "confirmed", "obsolete"] as const;
// Strength uses the amber "needs a person" tone, never the red conflict tone: a signal is not a finding.
const strengthTone = (s: string): Tone => (s === "strong" ? "warn" : "neutral");

function useGuard() {
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const guard = async (fn: () => Promise<void>) => {
    setErr(null);
    setMsg("");
    try {
      await fn();
    } catch (x) {
      setErr(x);
    }
  };
  return { msg, setMsg, err, guard };
}

/** Reason box shared by both views: a written reason is required except when asking for evidence. */
function ReviewBox({ id, actions, onReview, note }: { id: string; actions: [string, string][]; onReview: (d: string, c: string) => void; note: string }) {
  const { t } = useI18n();
  const [comment, setComment] = useState("");
  const [need, setNeed] = useState(false);
  const submit = (d: string) => {
    if (d !== "request_evidence" && !comment.trim()) return setNeed(true);
    setNeed(false);
    onReview(d, comment.trim());
  };
  return (
    <section aria-labelledby={`${id}-act`} className="review-box">
      <h3 id={`${id}-act`}>{t("finding.recordReview")}</h3>
      <p className="muted">{note}</p>
      <label className="stack">
        {t("finding.comment")}
        <textarea value={comment} onChange={(e) => setComment(e.target.value)} aria-invalid={need} aria-describedby={`${id}-hint`} />
      </label>
      <p id={`${id}-hint`} className={need ? "notice crit" : "muted"} role={need ? "alert" : undefined}>
        {t("finding.commentHint")}
      </p>
      <div className="actions">
        {actions.map(([d, label]) => (
          <button key={d} onClick={() => submit(d)}>
            {label}
          </button>
        ))}
      </div>
    </section>
  );
}

function Reviews({ reviews }: { reviews: Review[] }) {
  const { fmtDate } = useI18n();
  if (reviews.length === 0) return null;
  return (
    <ul className="list">
      {reviews.map((r, i) => (
        <li key={i}>
          <Badge group="review.decision" value={r.decision} /> {r.comment} <span className="muted num">{fmtDate(r.decided_at)}</span>
        </li>
      ))}
    </ul>
  );
}

function Documents({ docs }: { docs: Doc[] }) {
  const { t } = useI18n();
  return (
    <section aria-labelledby="i-docs">
      <h3 id="i-docs">{t("integrity.documents")}</h3>
      <ul className="list">
        {docs.map((d) => (
          <li key={d.artifact_id}>
            {d.display_name}{" "}
            <code title={d.sha256} aria-label={`${t("docs.sha")} ${d.sha256}`}>
              {d.sha256.slice(0, 16)}…
            </code>
          </li>
        ))}
      </ul>
    </section>
  );
}

export function SignalDetailView({ d, canReview, onReview }: { d: SignalDetail; canReview: boolean; onReview: (decision: string, comment: string) => void }) {
  const { t, tv, lang, fmtDate } = useI18n();
  const s = d.signal;
  return (
    <article className="finding" aria-labelledby="sig-h">
      <header className="finding-head">
        <h2 id="sig-h">
          {tv("integrity.kind", s.kind)} — <span className="mono">{s.supplier_key}</span>
        </h2>
        <Badge group="integrity.status" value={s.status} tone={s.status === "open" ? "warn" : undefined} />
        <span className="muted num">{t("finding.detected", { at: fmtDate(s.detected_at) })}</span>
      </header>
      <section aria-labelledby="sig-why">
        <h3 id="sig-why">{t("finding.why")}</h3>
        <p>{lang === "ja" ? s.explanation.why_ja : s.explanation.why}</p>
        <p>
          {t("integrity.strength")}: <Badge group="integrity.strength" value={s.strength} tone={strengthTone(s.strength)} />{" "}
          <span className="muted">{t("integrity.strengthNote")}</span>
        </p>
      </section>
      <section aria-labelledby="sig-stat">
        <h3 id="sig-stat">{t("integrity.statistic")}</h3>
        <dl className="kv">
          {Object.entries(s.statistic).map(([k, v]) => (
            <div key={k} className="kv-row">
              <dt className="mono">{k}</dt>
              <dd className="mono">{typeof v === "string" ? v : JSON.stringify(v)}</dd>
            </div>
          ))}
        </dl>
      </section>
      <section aria-labelledby="sig-ev">
        <h3 id="sig-ev">{t("integrity.evidence")}</h3>
        <div className="table-wrap">
          <table>
          <caption className="sr-only">{t("integrity.evidence")}</caption>
          <thead>
            <tr>
              <th scope="col">{t("finding.document")}</th>
              <th scope="col">{t("integrity.field")}</th>
              <th scope="col">{t("integrity.value")}</th>
              <th scope="col">{t("finding.location")}</th>
            </tr>
          </thead>
          <tbody>
            {s.evidence.map((e, i) => (
              <tr key={i}>
                <td>{e.document}</td>
                <td className="mono">{e.field}</td>
                <td className="mono num">{e.value}</td>
                <td>{sourceLocation(t, e.location)}</td>
              </tr>
            ))}
          </tbody>
        </table>
        </div>
      </section>
      <Documents docs={d.documents} />
      <section aria-labelledby="sig-lim">
        <h3 id="sig-lim">{t("finding.confidence")}</h3>
        <p className="muted">{d.limitations}</p>
      </section>
      <section aria-labelledby="sig-rev">
        <h3 id="sig-rev">{t("finding.reviewNeeded")}</h3>
        <p className={d.requires_human_review ? "callout warn" : "callout"}>{d.requires_human_review ? t("finding.reviewYes") : t("finding.reviewNo")}</p>
        <Reviews reviews={d.reviews} />
      </section>
      {d.requires_human_review && canReview && (
        <ReviewBox
          id="sig"
          note={t("integrity.neverReject")}
          onReview={onReview}
          actions={[
            ["explained", t("integrity.explained")],
            ["follow_up", t("integrity.followUp")],
            ["request_evidence", t("finding.requestEvidence")],
          ]}
        />
      )}
    </article>
  );
}

export function IntegrityView() {
  const { api, can } = useSession();
  const { t, tv, lang } = useI18n();
  const [status, setStatus] = useState("open");
  const list = useLoad(() => api.request<IntegritySignal[]>("GET", `/integrity/signals?status=${status}`), [status]);
  const [detail, setDetail] = useState<SignalDetail | null>(null);
  const { msg, setMsg, err, guard } = useGuard();
  const fetchDetail = async (id: string) => setDetail(await api.request<SignalDetail>("GET", `/integrity/signals/${id}`));
  const run = () =>
    guard(async () => {
      const r = await api.request<{ signals_new: number; drift_new: number; marked_obsolete: number }>("POST", "/integrity/run", {});
      setMsg(t("integrity.ran", { signals: r.signals_new, drift: r.drift_new, obsolete: r.marked_obsolete }));
      list.reload();
    });
  const review = (decision: string, comment: string) =>
    guard(async () => {
      if (!detail) return;
      await api.request("POST", `/integrity/signals/${detail.signal.id}/review`, { decision, comment: comment || null });
      setMsg(t("integrity.reviewed"));
      await fetchDetail(detail.signal.id);
      list.reload();
    });
  return (
    <div>
      <PageTitle title={t("nav.integrity")} lead={t("integrity.lead")} />
      <div className="toolbar">
        <label>
          {t("findings.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            {SIGNAL_STATUSES.map((s) => (
              <option key={s} value={s}>
                {tv("integrity.status", s)}
              </option>
            ))}
          </select>
        </label>
        {can("claim_write") && (
          <button className="primary" onClick={run}>
            {t("integrity.run")}
          </button>
        )}
      </div>
      <Notice msg={msg} error={err ?? list.error} />
      <div className="grid2 wide-right">
        <Panel title={t("integrity.list")} id="i-list">
          <Loading on={list.loading} />
          {list.data && list.data.length === 0 && <Empty text={t("integrity.none")} />}
          {list.data && list.data.length > 0 && (
            <DataTable
              label={t("integrity.list")}
              rows={list.data}
              rowKey={(s) => s.id}
              selectedKey={detail?.signal.id}
              onActivate={(s) => guard(() => fetchDetail(s.id))}
              cols={[
                { key: "kind", label: t("integrity.kind"), value: (s) => tv("integrity.kind", s.kind), render: (s) => <Badge group="integrity.kind" value={s.kind} tone="info" /> },
                {
                  key: "supplier",
                  label: t("integrity.supplier"),
                  value: (s) => s.supplier_key,
                  render: (s) => (
                    <button className="link mono" onClick={(e) => (e.stopPropagation(), guard(() => fetchDetail(s.id)))}>
                      {s.supplier_key}
                    </button>
                  ),
                },
                { key: "strength", label: t("integrity.strength"), value: (s) => s.strength, render: (s) => <Badge group="integrity.strength" value={s.strength} tone={strengthTone(s.strength)} /> },
                { key: "why", label: t("finding.why"), value: (s) => s.explanation.why, render: (s) => <span className="muted clamp">{lang === "ja" ? s.explanation.why_ja : s.explanation.why}</span> },
              ]}
            />
          )}
        </Panel>
        <div>{detail ? <SignalDetailView d={detail} onReview={review} canReview={can("review_decide")} /> : <Empty text={t("integrity.select")} />}</div>
      </div>
    </div>
  );
}

export function DriftDetailView({ d, canReview, onReview }: { d: DriftDetail; canReview: boolean; onReview: (decision: string, comment: string) => void }) {
  const { t, tv, lang, fmtDate } = useI18n();
  const f = d.drift;
  const docName = (id: string) => d.documents.find((x) => x.artifact_id === id)?.display_name ?? id.slice(0, 8);
  const cell = (sides: Side[]) =>
    sides.length === 0 ? (
      <span className="muted">{t("common.none")}</span>
    ) : (
      sides.map((s) => (
        <div key={s.claim}>
          <b className="mono num">{s.value}</b> <span className="muted">{sourceLocation(t, s.location)}</span>
        </div>
      ))
    );
  return (
    <article className="finding" aria-labelledby="drift-h">
      <header className="finding-head">
        <h2 id="drift-h">
          {tv("drift.kind", f.kind)} — <span className="mono">{f.drawing_key}</span>
        </h2>
        <Badge group="drift.status" value={f.status} tone={f.status === "open" ? "warn" : undefined} />
        <span className="muted num">{t("finding.detected", { at: fmtDate(f.detected_at) })}</span>
      </header>
      <section aria-labelledby="drift-why">
        <h3 id="drift-why">{t("finding.why")}</h3>
        <p>{lang === "ja" ? f.explanation.why_ja : f.explanation.why}</p>
        <p>
          {t("drift.revisions")}: <span className="mono">{f.earlier_revision ?? "—"}</span> ({docName(f.earlier_artifact)}) → <span className="mono">{f.later_revision ?? "—"}</span> ({docName(f.later_artifact)}) ·{" "}
          {t("integrity.strength")}: <Badge group="integrity.strength" value={f.strength} tone={strengthTone(f.strength)} />
        </p>
      </section>
      <section aria-labelledby="drift-diff">
        <h3 id="drift-diff">{t("drift.diff")}</h3>
        {f.changes.length === 0 ? (
          <p className="muted">{t("common.none")}</p>
        ) : (
          <div className="table-wrap">
          <table>
            <caption className="sr-only">{t("drift.diff")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("integrity.field")}</th>
                <th scope="col">{t("drift.change")}</th>
                <th scope="col">{t("drift.before")}</th>
                <th scope="col">{t("drift.after")}</th>
              </tr>
            </thead>
            <tbody>
              {f.changes.map((c) => (
                <tr key={c.predicate}>
                  <td className="mono">{c.predicate}</td>
                  <td>
                    <Badge group="drift.changeKind" value={c.change} tone={c.change === "changed" ? "warn" : "neutral"} />
                  </td>
                  <td>{cell(c.before)}</td>
                  <td>{cell(c.after)}</td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        )}
      </section>
      <Documents docs={d.documents} />
      <section aria-labelledby="drift-lim">
        <h3 id="drift-lim">{t("finding.confidence")}</h3>
        <p className="muted">{d.limitations}</p>
      </section>
      <section aria-labelledby="drift-rev">
        <h3 id="drift-rev">{t("finding.reviewNeeded")}</h3>
        <p className={d.requires_human_review ? "callout warn" : "callout"}>{d.requires_human_review ? t("finding.reviewYes") : t("finding.reviewNo")}</p>
        <Reviews reviews={d.reviews} />
      </section>
      <section aria-labelledby="drift-aff">
        <h3 id="drift-aff">{t("finding.affected")}</h3>
        {d.potentially_affected_decisions.length === 0 ? (
          <p className="muted">{t("finding.noAffected")}</p>
        ) : (
          <ul className="list">
            {d.potentially_affected_decisions.map((x) => (
              <li key={x.id}>
                {x.title} <Badge group="decision.status" value={x.status} />
              </li>
            ))}
          </ul>
        )}
      </section>
      {d.requires_human_review && canReview && (
        <ReviewBox
          id="drift"
          note={t("drift.confirmNote")}
          onReview={onReview}
          actions={[
            ["accept_change", t("drift.acceptChange")],
            ["confirm_drift", t("drift.confirm")],
            ["request_evidence", t("finding.requestEvidence")],
          ]}
        />
      )}
    </article>
  );
}

export function DriftView() {
  const { api, can } = useSession();
  const { t, tv, lang } = useI18n();
  const [status, setStatus] = useState("open");
  const list = useLoad(() => api.request<DriftFinding[]>("GET", `/integrity/drift?status=${status}`), [status]);
  const [detail, setDetail] = useState<DriftDetail | null>(null);
  const { msg, setMsg, err, guard } = useGuard();
  const fetchDetail = async (id: string) => setDetail(await api.request<DriftDetail>("GET", `/integrity/drift/${id}`));
  const run = () =>
    guard(async () => {
      const r = await api.request<{ signals_new: number; drift_new: number; marked_obsolete: number }>("POST", "/integrity/run", {});
      setMsg(t("integrity.ran", { signals: r.signals_new, drift: r.drift_new, obsolete: r.marked_obsolete }));
      list.reload();
    });
  const review = (decision: string, comment: string) =>
    guard(async () => {
      if (!detail) return;
      const r = await api.request<{ decisions_flagged: number }>("POST", `/integrity/drift/${detail.drift.id}/review`, { decision, comment: comment || null });
      setMsg(decision === "confirm_drift" ? t("drift.flagged", { n: r.decisions_flagged }) : t("integrity.reviewed"));
      await fetchDetail(detail.drift.id);
      list.reload();
    });
  return (
    <div>
      <PageTitle title={t("nav.drift")} lead={t("drift.lead")} />
      <div className="toolbar">
        <label>
          {t("findings.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            {DRIFT_STATUSES.map((s) => (
              <option key={s} value={s}>
                {tv("drift.status", s)}
              </option>
            ))}
          </select>
        </label>
        {can("claim_write") && (
          <button className="primary" onClick={run}>
            {t("integrity.run")}
          </button>
        )}
      </div>
      <Notice msg={msg} error={err ?? list.error} />
      <div className="grid2 wide-right">
        <Panel title={t("drift.list")} id="d-list">
          <Loading on={list.loading} />
          {list.data && list.data.length === 0 && <Empty text={t("drift.none")} />}
          {list.data && list.data.length > 0 && (
            <DataTable
              label={t("drift.list")}
              rows={list.data}
              rowKey={(f) => f.id}
              selectedKey={detail?.drift.id}
              onActivate={(f) => guard(() => fetchDetail(f.id))}
              cols={[
                { key: "kind", label: t("integrity.kind"), value: (f) => tv("drift.kind", f.kind), render: (f) => <Badge group="drift.kind" value={f.kind} tone="info" /> },
                {
                  key: "drawing",
                  label: t("drift.drawing"),
                  value: (f) => f.drawing_key,
                  render: (f) => (
                    <button className="link mono" onClick={(e) => (e.stopPropagation(), guard(() => fetchDetail(f.id)))}>
                      {f.drawing_key}
                    </button>
                  ),
                },
                { key: "revs", label: t("drift.revisions"), value: (f) => `${f.earlier_revision ?? ""}→${f.later_revision ?? ""}`, render: (f) => <span className="mono">{`${f.earlier_revision ?? "—"} → ${f.later_revision ?? "—"}`}</span> },
                { key: "strength", label: t("integrity.strength"), value: (f) => f.strength, render: (f) => <Badge group="integrity.strength" value={f.strength} tone={strengthTone(f.strength)} /> },
                { key: "why", label: t("finding.why"), value: (f) => f.explanation.why, render: (f) => <span className="muted clamp">{lang === "ja" ? f.explanation.why_ja : f.explanation.why}</span> },
              ]}
            />
          )}
        </Panel>
        <div>{detail ? <DriftDetailView d={detail} onReview={review} canReview={can("review_decide")} /> : <Empty text={t("drift.select")} />}</div>
      </div>
    </div>
  );
}
