import { useMemo, useState } from "react";
import { claimValue, confidenceShort, sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { AiBadge, ClaimId, ClaimReview } from "./claims";
import type { Claim, EntityCandidate, PredicateEvidence, Subject } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, short, useLoad } from "./ui";

export function EvidenceTable({
  groups,
  onReview,
  reviewing,
  onSupersede,
}: {
  groups: PredicateEvidence[];
  onReview?: (c: Claim) => void;
  reviewing?: string;
  onSupersede?: (c: Claim) => void;
}) {
  const { t, tv } = useI18n();
  if (!groups.length) return <Empty text={t("evidence.none")} />;
  return (
    <>
    <div className="table-wrap">
      <table>
        <thead>
          <tr>
            <th scope="col">{t("evidence.predicate")}</th>
            <th scope="col">{t("evidence.value")}</th>
            <th scope="col">{t("evidence.revision")}</th>
            <th scope="col">{t("evidence.source")}</th>
            <th scope="col">{t("evidence.method")}</th>
            <th scope="col">{t("evidence.review")}</th>
            <th scope="col">{t("claim.id")}</th>
            {onReview && (
              <th scope="col">
                <span className="sr-only">{t("common.actions")}</span>
              </th>
            )}
          </tr>
        </thead>
        <tbody>
          {groups.flatMap((g) =>
            g.current.map((c, i) => (
              <tr key={c.id} className={g.disagreement ? "row-crit" : undefined} aria-current={reviewing === c.id ? "true" : undefined}>
                <td>
                  {i === 0 && <span className="mono">{g.predicate}</span>}
                  {i === 0 && g.disagreement && <span className="badge crit">{t("evidence.disagree")}</span>}
                  {i === 0 && g.superseded_count > 0 && <span className="badge info">{t("evidence.superseded", { n: g.superseded_count })}</span>}
                </td>
                <td className="mono">{claimValue(c)}</td>
                <td className="mono">{c.revision ?? "—"}</td>
                <td>
                  {c.artifact_id ? (c.artifact_display_name ?? t("evidence.artifactRef", { id: short(c.artifact_id) })) : t("evidence.manual")} · {sourceLocation(t, c.source_location)}
                </td>
                <td>
                  {tv("method", c.extraction_method)} · <span className="num">{confidenceShort(t, c.confidence_bps)}</span>
                </td>
                <td>
                  <Badge group="review" value={c.review_status} /> <AiBadge c={c} />
                </td>
                <td>
                  <ClaimId id={c.id} />
                </td>
                {onReview && (
                  <td className="col-actions">
                    <button onClick={() => onReview(c)} aria-label={t("claim.reviewFor", { predicate: c.predicate, value: claimValue(c) })}>
                      {t(c.review_status === "ai_suggested" ? "claim.reviewAi" : "claim.review")}
                    </button>
                    {onSupersede && (
                      <button
                        className="link"
                        aria-label={t("evidence.supersedeFor", { predicate: c.predicate, value: claimValue(c) })}
                        onClick={() => onSupersede(c)}
                      >
                        {t("evidence.supersede")}
                      </button>
                    )}
                  </td>
                )}
              </tr>
            )),
          )}
        </tbody>
      </table>
    </div>
    <p className="hint table-note">{t("evidence.confidenceNote")}</p>
    </>
  );
}

type RecordType = "supplier" | "part";
const PAGE = 200;

function SupersedeForm({ c, onDone }: { c: Claim; onDone: (ok: boolean) => void }) {
  const { api } = useSession();
  const { t } = useI18n();
  const [f, setF] = useState({ value: "", unit: c.value_unit ?? "", reason: "" });
  const [err, setErr] = useState<unknown>(null);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    const v = f.value.trim();
    const numeric = c.value_num !== null && /^-?\d+(\.\d+)?$/.test(v);
    try {
      await api.request("POST", `/claims/${c.id}/supersede`, {
        reason: f.reason.trim(),
        subject_type: c.subject_type,
          subject_key: c.subject_key,
          predicate: c.predicate,
          value_text: numeric ? null : v,
          value_num: numeric ? v : null,
          value_unit: numeric ? f.unit.trim() || null : null,
          revision: c.revision,
          artifact_id: c.artifact_id,
          source_location: c.source_location,
      });
      onDone(true);
    } catch (x) {
      setErr(x);
    }
  }
  return (
    <form onSubmit={submit} className="review-box" aria-labelledby="sup-h">
      <h3 id="sup-h">{t("evidence.supersedeFor", { predicate: c.predicate, value: claimValue(c) })}</h3>
      <div className="form-grid">
        <label>
          {t("evidence.newValue")}
          <input required value={f.value} onChange={(e) => setF({ ...f, value: e.target.value })} />
        </label>
        {c.value_num !== null && (
          <label>
            {t("evidence.newUnit")}
            <input value={f.unit} onChange={(e) => setF({ ...f, unit: e.target.value })} />
          </label>
        )}
        <label>
          {t("evidence.reason")}
          <input required maxLength={500} value={f.reason} onChange={(e) => setF({ ...f, reason: e.target.value })} />
        </label>
      </div>
      <Notice error={err} />
      <div className="actions">
        <button className="primary">{t("evidence.supersedeBtn")}</button>
        <button type="button" onClick={() => onDone(false)}>
          {t("common.cancel")}
        </button>
      </div>
    </form>
  );
}


export function RecordsView() {
  const { api, can } = useSession();
  const [sup, setSup] = useState<Claim | null>(null);
  const [supMsg, setSupMsg] = useState("");
  const { t } = useI18n();
  const [type, setType] = useState<RecordType>("part");
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const [reviewing, setReviewing] = useState<Claim | null>(null);
  // Server-side distinct records with counts, paged by subject_key (keyset).
  const [rows, setRows] = useState<Subject[]>([]);
  const [moreErr, setMoreErr] = useState<unknown>(null);
  const [done, setDone] = useState(false);
  const page = (after?: string) =>
    api.request<Subject[]>("GET", `/subjects?type=${type}&limit=${PAGE}${after ? `&after=${encodeURIComponent(after)}` : ""}`);
  const claims = useLoad(async () => {
    const r = await page();
    setRows(r);
    return r;
  }, [type]);
  async function more() {
    setMoreErr(null);
    try {
      const r = await page(rows[rows.length - 1]?.subject_key);
      setRows((x) => [...x, ...r]);
      if (r.length < PAGE) setDone(true);
    } catch (x) {
      setMoreErr(x);
    }
  }
  const evidence = useLoad<PredicateEvidence[] | null>(
    async () =>
      selected
        ? (await api.request<{ predicates: PredicateEvidence[] }>("GET", `/evidence/${type}/${encodeURIComponent(selected)}`)).predicates
        : null,
    [type, selected],
  );

  const records = useMemo(() => {
    const f = filter.trim().toLowerCase();
    return rows.filter((r) => !f || r.subject_key.toLowerCase().includes(f));
  }, [rows, filter]);
  const hasMore = !done && (claims.data?.length ?? 0) === PAGE;

  return (
    <div>
      <PageTitle title={t("nav.records")} lead={t("records.lead")} />
      <div role="tablist" aria-label={t("records.type")} className="seg">
        {(["part", "supplier"] as const).map((x) => (
          <button
            key={x}
            role="tab"
            aria-selected={type === x}
            onClick={() => {
              setType(x);
              setSelected(null);
              setDone(false);
            }}
          >
            {t(x === "part" ? "records.components" : "records.suppliers")}
          </button>
        ))}
      </div>
      <div className="grid2 wide-right">
        <Panel title={t(type === "part" ? "records.components" : "records.suppliers")} id="rec-list">
          <label className="stack">
            {t("records.filter")}
            <input value={filter} onChange={(e) => setFilter(e.target.value)} />
          </label>
          <Loading on={claims.loading} />
          <Notice error={claims.error ?? moreErr} />
          {claims.data && records.length === 0 && <Empty text={t("records.none")} />}
          <ul className="list selectable">
            {records.map((r) => (
              <li key={r.subject_key}>
                <button aria-pressed={selected === r.subject_key} onClick={() => setSelected(r.subject_key)}>
                  <span className="mono">{r.subject_key}</span>
                  <span className="muted">{t("records.summary", { claims: r.claims, fields: r.predicates })}</span>
                </button>
              </li>
            ))}
          </ul>
          {hasMore && (
            <button className="link" onClick={more}>
              {t("records.more")}
            </button>
          )}
        </Panel>
        <Panel title={selected ? t("records.evidenceFor", { key: selected }) : t("records.selectPrompt")} id="rec-ev">
          <Loading on={!!selected && evidence.loading} />
          <Notice error={evidence.error} />
          <Notice msg={supMsg} />
          {evidence.data && (
            <EvidenceTable
              groups={evidence.data}
              onReview={setReviewing}
              reviewing={reviewing?.id}
              onSupersede={can("claim_write") ? (c) => (setSupMsg(""), setSup(c)) : undefined}
            />
          )}
          {sup && sup.subject_key === selected && (
            <SupersedeForm
              c={sup}
              onDone={(ok) => {
                setSup(null);
                if (ok) {
                  setSupMsg(t("evidence.supersededMsg"));
                  evidence.reload();
                }
              }}
            />
          )}
          {reviewing && reviewing.subject_key === selected && (
            <ClaimReview
              claim={reviewing}
              onReviewed={(c) => {
                setReviewing(c);
                evidence.reload();
              }}
              onClose={() => setReviewing(null)}
            />
          )}
        </Panel>
      </div>
      <DuplicateCandidates key={type} type={type} />
    </div>
  );
}

function DuplicateCandidates({ type }: { type: RecordType }) {
  const { api, can } = useSession();
  const { t, tv } = useI18n();
  const cands = useLoad(() => api.request<{ candidates: EntityCandidate[] }>("GET", `/entities/candidates?subject_type=${type}`), [type]);
  const [err, setErr] = useState<unknown>(null);
  async function decide(c: EntityCandidate, outcome: "same" | "different") {
    setErr(null);
    try {
      await api.request("POST", "/entities/decisions", { subject_type: type, left_key: c.left, right_key: c.right, outcome });
      cands.reload();
    } catch (x) {
      setErr(x);
    }
  }
  const list = cands.data?.candidates ?? [];
  return (
    <Panel title={t("records.duplicates")} id="dups">
      <p className="muted">{t(type === "part" ? "records.duplicatesNote" : "records.duplicatesNoteSupplier")}</p>
      <Notice error={err ?? cands.error} />
      {cands.data && list.length === 0 && <Empty text={t("records.noDuplicates")} />}
      {list.length > 0 && (
        <div className="table-wrap" tabIndex={0} role="region" aria-label={t("records.duplicatesTable")}>
          <table>
            <thead>
              <tr>
                <th scope="col">{t("records.left")}</th>
                <th scope="col">{t("records.right")}</th>
                <th scope="col">{t("records.assessment")}</th>
                <th scope="col">{t("records.reasons")}</th>
                <th scope="col">
                  <span className="sr-only">{t("common.actions")}</span>
                </th>
              </tr>
            </thead>
            <tbody>
              {list.map((c) => (
                <tr key={`${c.left}|${c.right}`}>
                  <td className="mono">{c.left}</td>
                  <td className="mono">{c.right}</td>
                  <td>
                    <Badge group="entity" value={c.outcome} />
                  </td>
                  <td>{c.reasons.map((r) => tv("entity.reason", r)).join(", ")}</td>
                  <td className="col-actions">
                    {can("review_decide") && (
                      <>
                        <button onClick={() => decide(c, "same")}>{t(type === "part" ? "records.same" : "records.sameSupplier")}</button>
                        <button onClick={() => decide(c, "different")}>{t(type === "part" ? "records.different" : "records.differentSupplier")}</button>
                      </>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
    </Panel>
  );
}
