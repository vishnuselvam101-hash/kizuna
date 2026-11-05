import { useState } from "react";
import { DataTable } from "./DataTable";
import { claimValue, confidence, confidenceShort, sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Claim, Conflict, ConflictDetail } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";

const STATUSES = ["open", "needs_evidence", "resolved_approved", "resolved_rejected", "obsolete"] as const;

function SourceCard({ c, d }: { c: Claim; d: ConflictDetail }) {
  const { t, tv } = useI18n();
  const src = d.sources.find((s) => s.claim_id === c.id);
  return (
    <div className="source">
      <p className="source-value">
        <span className="mono">{c.predicate}</span> = <b className="mono">{claimValue(c)}</b>
        {c.revision && <span className="badge info">{t("evidence.revShort", { rev: c.revision })}</span>}
      </p>
      <dl className="kv">
        <dt>{t("finding.document")}</dt>
        <dd>{src ? src.display_name : t("evidence.manual")}</dd>
        <dt>{t("finding.location")}</dt>
        <dd>{sourceLocation(t, src?.location ?? c.source_location)}</dd>
        {src?.sha256 && (
          <>
            <dt>{t("docs.sha")}</dt>
            <dd>
              <code title={src.sha256}>{src.sha256.slice(0, 16)}…</code>
            </dd>
          </>
        )}
        <dt>{t("evidence.method")}</dt>
        <dd>
          {tv("method", c.extraction_method)} · <span className="num">{confidenceShort(t, c.confidence_bps)}</span>
        </dd>
        <dt>{t("evidence.review")}</dt>
        <dd>
          <Badge group="review" value={c.review_status} />
        </dd>
      </dl>
    </div>
  );
}

/** Every finding answers the seven questions of DIRECTIVE §12; no section is ever silently omitted. */
export function FindingDetail({ d, onReview, canReview }: { d: ConflictDetail; onReview: (decision: string, comment: string) => void; canReview: boolean }) {
  const { t, tv, fmtDate } = useI18n();
  const [comment, setComment] = useState("");
  const [needComment, setNeedComment] = useState(false);
  const c = d.conflict;
  const extra = Object.entries(c.explanation).filter(([k]) => k !== "why");
  const affected = d.potentially_affected_decisions ?? [];
  function submit(decision: string) {
    if (decision !== "request_evidence" && !comment.trim()) {
      setNeedComment(true);
      return;
    }
    setNeedComment(false);
    onReview(decision, comment.trim());
  }
  return (
    <article className="finding" aria-labelledby="finding-h">
      <header className="finding-head">
        <h2 id="finding-h">
          {tv("conflict.kind", c.kind)} — <span className="mono">{c.subject_key}</span>
        </h2>
        <Badge group="conflict.status" value={c.status} />
        <span className="muted num">{t("finding.detected", { at: fmtDate(c.detected_at) })}</span>
      </header>
      <section aria-labelledby="f-what">
        <h3 id="f-what">{t("finding.what")}</h3>
        <p>{t("finding.whatText", { kind: tv("conflict.kind", c.kind), key: c.subject_key, predicate: d.claim_a.predicate })}</p>
      </section>
      <section aria-labelledby="f-why">
        <h3 id="f-why">{t("finding.why")}</h3>
        <p>{c.explanation.why ?? t("finding.noWhy")}</p>
        {extra.length > 0 && (
          <dl className="kv">
            {extra.map(([k, v]) => (
              <div key={k} className="kv-row">
                <dt className="mono">{k}</dt>
                <dd className="mono">{typeof v === "string" ? v : JSON.stringify(v)}</dd>
              </div>
            ))}
          </dl>
        )}
      </section>
      <div className="grid2">
        <section aria-labelledby="f-sup" className="evidence-col ok-edge">
          <h3 id="f-sup">{t("finding.supporting")}</h3>
          <SourceCard c={d.claim_a} d={d} />
        </section>
        <section aria-labelledby="f-con" className="evidence-col crit-edge">
          <h3 id="f-con">{t("finding.contradicting")}</h3>
          {d.claim_b ? <SourceCard c={d.claim_b} d={d} /> : <p className="muted">{t("finding.noContradicting")}</p>}
        </section>
      </div>
      <section aria-labelledby="f-conf">
        <h3 id="f-conf">{t("finding.confidence")}</h3>
        <p>
          {t("finding.lowestConfidence", {
            pct: confidence(t, Math.min(d.claim_a.confidence_bps, d.claim_b?.confidence_bps ?? d.claim_a.confidence_bps)),
          })}
        </p>
        <p className="muted">{d.limitations}</p>
      </section>
      <section aria-labelledby="f-rev">
        <h3 id="f-rev">{t("finding.reviewNeeded")}</h3>
        <p className={d.requires_human_review ? "callout crit" : "callout"}>
          {d.requires_human_review ? t("finding.reviewYes") : t("finding.reviewNo")}
        </p>
        {d.reviews.length > 0 && (
          <ul className="list">
            {d.reviews.map((r, i) => (
              <li key={i}>
                <Badge group="review.decision" value={r.decision} /> {r.comment} <span className="muted num">{fmtDate(r.decided_at)}</span>
              </li>
            ))}
          </ul>
        )}
      </section>
      <section aria-labelledby="f-aff">
        <h3 id="f-aff">{t("finding.affected")}</h3>
        {affected.length === 0 ? (
          <p className="muted">{t("finding.noAffected")}</p>
        ) : (
          <ul className="list">
            {affected.map((x) => (
              <li key={x.id}>
                {x.title} <Badge group="decision.status" value={x.status} />
              </li>
            ))}
          </ul>
        )}
      </section>
      {d.requires_human_review && canReview && (
        <section aria-labelledby="f-act" className="review-box">
          <h3 id="f-act">{t("finding.recordReview")}</h3>
          <label className="stack">
            {t("finding.comment")}
            <textarea
              value={comment}
              onChange={(e) => setComment(e.target.value)}
              aria-invalid={needComment}
              aria-describedby="f-comment-hint"
            />
          </label>
          <p id="f-comment-hint" className={needComment ? "notice crit" : "muted"} role={needComment ? "alert" : undefined}>
            {t("finding.commentHint")}
          </p>
          <div className="actions">
            <button onClick={() => submit("approve")}>{t("finding.approve")}</button>
            <button className="danger" onClick={() => submit("reject")}>
              {t("finding.reject")}
            </button>
            <button onClick={() => submit("request_evidence")}>{t("finding.requestEvidence")}</button>
          </div>
        </section>
      )}
    </article>
  );
}

export function FindingsView() {
  const { api, can } = useSession();
  const { t, tv } = useI18n();
  const [status, setStatus] = useState<string>("open");
  const list = useLoad(() => api.request<Conflict[]>("GET", `/conflicts?status=${status}`), [status]);
  const [detail, setDetail] = useState<ConflictDetail | null>(null);
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
  const fetchDetail = async (id: string) => setDetail(await api.request<ConflictDetail>("GET", `/conflicts/${id}`));
  const open = (id: string) => guard(() => fetchDetail(id));
  const run = () =>
    guard(async () => {
      const r = await api.request<{ findings_new: number; marked_obsolete: number }>("POST", "/verification/run", {});
      setMsg(t("findings.ran", { n: r.findings_new, obsolete: r.marked_obsolete }));
      list.reload();
    });
  const review = (decision: string, comment: string) =>
    guard(async () => {
      if (!detail) return;
      await api.request("POST", `/conflicts/${detail.conflict.id}/review`, { decision, comment: comment || null });
      setMsg(t("findings.reviewed"));
      await fetchDetail(detail.conflict.id);
      list.reload();
    });

  return (
    <div>
      <PageTitle title={t("nav.findings")} lead={t("findings.lead")} />
      <div className="toolbar">
        <label>
          {t("findings.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {tv("conflict.status", s)}
              </option>
            ))}
          </select>
        </label>
        {can("claim_write") && (
          <button className="primary" onClick={run}>
            {t("findings.run")}
          </button>
        )}
      </div>
      <Notice msg={msg} error={err ?? list.error} />
      <div className="grid2 wide-right">
        <Panel title={t("findings.list")} id="f-list">
          <Loading on={list.loading} />
          {list.data && list.data.length === 0 && <Empty text={t("findings.none")} />}
          {list.data && list.data.length > 0 && (
            <DataTable
              label={t("findings.list")}
              rows={list.data}
              rowKey={(c) => c.id}
              selectedKey={detail?.conflict.id}
              onActivate={(c) => open(c.id)}
              cols={[
                {
                  key: "kind",
                  label: t("findings.kind"),
                  value: (c) => tv("conflict.kind", c.kind),
                  render: (c) => <Badge group="conflict.kind" value={c.kind} tone={c.status === "open" ? "crit" : "info"} />,
                },
                {
                  key: "subject",
                  label: t("compare.part"),
                  value: (c) => c.subject_key,
                  render: (c) => (
                    <button className="link mono" onClick={(e) => (e.stopPropagation(), open(c.id))}>
                      {c.subject_key}
                    </button>
                  ),
                },
                { key: "why", label: t("finding.why"), value: (c) => c.explanation.why ?? "", render: (c) => <span className="muted clamp">{c.explanation.why}</span> },
              ]}
            />
          )}
        </Panel>
        <div>{detail ? <FindingDetail d={detail} onReview={review} canReview={can("review_decide")} /> : <Empty text={t("findings.select")} />}</div>
      </div>
    </div>
  );
}
