import { useMemo, useState } from "react";
import { claimValue, confidence, sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Artifact, Claim, EntityCandidate, PredicateEvidence } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, short, useLoad } from "./ui";

export function EvidenceTable({ groups, names }: { groups: PredicateEvidence[]; names?: Map<string, string> }) {
  const { t, tv } = useI18n();
  if (!groups.length) return <Empty text={t("evidence.none")} />;
  return (
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
          </tr>
        </thead>
        <tbody>
          {groups.flatMap((g) =>
            g.current.map((c, i) => (
              <tr key={c.id} className={g.disagreement ? "row-crit" : undefined}>
                <td>
                  {i === 0 && <span className="mono">{g.predicate}</span>}
                  {i === 0 && g.disagreement && <span className="badge crit">{t("evidence.disagree")}</span>}
                  {i === 0 && g.superseded_count > 0 && <span className="badge info">{t("evidence.superseded", { n: g.superseded_count })}</span>}
                </td>
                <td className="mono">{claimValue(c)}</td>
                <td className="mono">{c.revision ?? "—"}</td>
                <td>
                  {c.artifact_id ? (names?.get(c.artifact_id) ?? t("evidence.artifactRef", { id: short(c.artifact_id) })) : t("evidence.manual")} · {sourceLocation(t, c.source_location)}
                </td>
                <td>
                  {tv("method", c.extraction_method)} · {confidence(t, c.confidence_bps)}
                </td>
                <td>
                  <Badge group="review" value={c.review_status} />
                </td>
              </tr>
            )),
          )}
        </tbody>
      </table>
    </div>
  );
}

type RecordType = "supplier" | "part";

export function RecordsView() {
  const { api } = useSession();
  const { t } = useI18n();
  const [type, setType] = useState<RecordType>("part");
  const [filter, setFilter] = useState("");
  const [selected, setSelected] = useState<string | null>(null);
  const claims = useLoad(() => api.request<Claim[]>("GET", `/claims?subject_type=${type}`), [type]);
  const evidence = useLoad<PredicateEvidence[] | null>(
    async () =>
      selected
        ? (await api.request<{ predicates: PredicateEvidence[] }>("GET", `/evidence/${type}/${encodeURIComponent(selected)}`)).predicates
        : null,
    [type, selected],
  );

  const artifacts = useLoad(() => api.request<Artifact[]>("GET", "/artifacts"), []);
  const names = useMemo(() => new Map((artifacts.data ?? []).map((a) => [a.id, a.display_name])), [artifacts.data]);

  const records = useMemo(() => {
    const m = new Map<string, { claims: number; revisions: Set<string>; unreviewed: number }>();
    for (const c of claims.data ?? []) {
      if (c.superseded_at) continue;
      const r = m.get(c.subject_key) ?? { claims: 0, revisions: new Set(), unreviewed: 0 };
      r.claims++;
      if (c.revision) r.revisions.add(c.revision);
      if (c.review_status === "unreviewed") r.unreviewed++;
      m.set(c.subject_key, r);
    }
    const f = filter.trim().toLowerCase();
    return [...m.entries()].filter(([k]) => !f || k.toLowerCase().includes(f)).sort(([a], [b]) => a.localeCompare(b));
  }, [claims.data, filter]);

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
          <Notice error={claims.error} />
          {claims.data && records.length === 0 && <Empty text={t("records.none")} />}
          <ul className="list selectable">
            {records.map(([k, r]) => (
              <li key={k}>
                <button aria-pressed={selected === k} onClick={() => setSelected(k)}>
                  <span className="mono">{k}</span>
                  <span className="muted">
                    {t("records.summary", { claims: r.claims, revs: [...r.revisions].join(", ") || "—" })}
                  </span>
                  {r.unreviewed > 0 && <span className="badge warn">{t("records.unreviewed", { n: r.unreviewed })}</span>}
                </button>
              </li>
            ))}
          </ul>
        </Panel>
        <Panel title={selected ? t("records.evidenceFor", { key: selected }) : t("records.selectPrompt")} id="rec-ev">
          <Loading on={!!selected && evidence.loading} />
          <Notice error={evidence.error} />
          {evidence.data && <EvidenceTable groups={evidence.data} names={names} />}
        </Panel>
      </div>
      {type === "part" && <DuplicateCandidates />}
    </div>
  );
}

function DuplicateCandidates() {
  const { api, can } = useSession();
  const { t, tv } = useI18n();
  const cands = useLoad(() => api.request<{ candidates: EntityCandidate[] }>("GET", "/entities/candidates?subject_type=part"), []);
  const [err, setErr] = useState<unknown>(null);
  async function decide(c: EntityCandidate, outcome: "same" | "different") {
    setErr(null);
    try {
      await api.request("POST", "/entities/decisions", { subject_type: "part", left_key: c.left, right_key: c.right, outcome });
      cands.reload();
    } catch (x) {
      setErr(x);
    }
  }
  const list = cands.data?.candidates ?? [];
  return (
    <Panel title={t("records.duplicates")} id="dups">
      <p className="muted">{t("records.duplicatesNote")}</p>
      <Notice error={err ?? cands.error} />
      {cands.data && list.length === 0 && <Empty text={t("records.noDuplicates")} />}
      {list.length > 0 && (
        <div className="table-wrap">
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
                        <button onClick={() => decide(c, "same")}>{t("records.same")}</button>
                        <button onClick={() => decide(c, "different")}>{t("records.different")}</button>
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
