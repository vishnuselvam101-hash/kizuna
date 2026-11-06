import { useCallback } from "react";
import { GraphScene } from "./graph";
import type { GraphModel, GraphPage, PageFetcher } from "./graphmodel";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { ChainStatus, Conflict, DashboardSummary, Decision } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";
import type { View } from "./nav";

/** Claims per graph page; the scene streams pages until the server reports no `next`. */
const GRAPH_PAGE = 5000;

const PICK: Record<string, View> = { doc: "documents", claim: "records", subj: "records", dec: "decisions", fnd: "findings" };

interface Summary {
  counts: DashboardSummary;
  open: Conflict[];
  decisions: Decision[];
  chain: ChainStatus | null;
}

const sum = (m: Record<string, number> | undefined) => Object.values(m ?? {}).reduce((a, b) => a + b, 0);

export function DashboardView({ onNavigate }: { onNavigate: (v: View) => void }) {
  const { api, me, can } = useSession();
  const { tm, t, fmtNum } = useI18n();
  const { data, error, loading } = useLoad<Summary>(async () => {
    // Counts come from the server summary; the lists below only feed the graph and attention panels.
    const [counts, open, decisions, chain] = await Promise.all([
      api.request<DashboardSummary>("GET", "/dashboard/summary"),
      api.request<Conflict[]>("GET", "/conflicts?status=open"),
      api.request<Decision[]>("GET", "/decisions"),
      can("audit_verify") ? api.request<ChainStatus>("GET", "/audit/verify") : Promise.resolve(null),
    ]);
    return { counts, open, decisions, chain };
  }, []);

  const s = data;
  // Streamed from the paged graph endpoint (documents, decisions and findings on the first page).
  const pages = useCallback<PageFetcher>(
    (after) => api.request<GraphPage>("GET", `/graph/overview?limit=${GRAPH_PAGE}${after ? `&after=${encodeURIComponent(after)}` : ""}`),
    [api],
  );
  const graphLabel = (m: GraphModel) =>
    t("dash.graphLabel", { docs: m.count("document"), claims: m.count("claim"), subjects: m.count("subject"), decisions: m.count("decision"), findings: m.count("finding") });
  const affected = s?.decisions.filter((d) => d.status === "potentially_affected") ?? [];
  const c = s?.counts;
  const docs = sum(c?.artifacts);
  const quarantined = docs - (c?.artifacts.accepted ?? 0);
  const openN = c?.conflicts.open ?? 0;
  const affectedN = c?.decisions.potentially_affected ?? 0;
  const kpis = c
    ? [
        { k: "dash.kpi.documents", v: docs, sub: t("dash.kpi.notAccepted", { n: fmtNum(quarantined) }), go: "documents" as View, tone: "", subTone: quarantined ? "warn" : "" },
        { k: "dash.kpi.openFindings", v: openN, sub: t("dash.kpi.needsEvidence", { n: fmtNum(c.conflicts.needs_evidence ?? 0) }), go: "findings" as View, tone: openN ? "crit" : "" },
        { k: "dash.kpi.decisions", v: c.decisions.valid ?? 0, sub: t("dash.kpi.validOfTotal", { n: fmtNum(sum(c.decisions)) }), go: "decisions" as View, tone: "" },
        { k: "dash.kpi.affected", v: affectedN, sub: t("dash.kpi.affectedSub"), go: "affected" as View, tone: affectedN ? "crit" : "" },
      ]
    : [];

  return (
    <div>
      <PageTitle title={t("nav.dashboard")} lead={t("dash.lead", { name: me.display_name })} />
      <Loading on={loading} />
      <Notice error={error} />
      {s && (
        <>
          <ul className="kpis" aria-label={t("dash.kpisLabel")}>
            {kpis.map((k) => (
              <li key={k.k} className={`kpi ${k.tone}`}>
                <button className="kpi-btn" onClick={() => onNavigate(k.go)}>
                  <span className="kpi-label">{t(k.k as never)}</span>
                  <span className="kpi-value num">{fmtNum(k.v)}</span>
                  <span className={`kpi-sub ${"subTone" in k ? k.subTone : ""}`}>{k.sub}</span>
                </button>
              </li>
            ))}
            {s.chain && (
              <li className={`kpi ${s.chain.intact ? "ok" : "crit"}`}>
                <button className="kpi-btn" onClick={() => onNavigate("reports")}>
                  <span className="kpi-label">{t("dash.kpi.auditChain")}</span>
                  <span className="kpi-value">{s.chain.intact ? t("audit.intact") : t("audit.broken")}</span>
                  <span className="kpi-sub">{t("audit.checked", { n: fmtNum(s.chain.events_checked) })}</span>
                </button>
              </li>
            )}
          </ul>
          {(c?.claims && sum(c.claims) > 0) || docs > 0 ? (
            <Panel title={t("dash.graph")} id="dash-g">
              <p className="muted">{t("dash.graphLead")}</p>
              <GraphScene pages={pages} label={graphLabel} onPick={(id) => onNavigate(PICK[id.split(":")[0]] ?? "dashboard")} />
            </Panel>
          ) : null}
          <div className="grid2">
            <Panel title={t("dash.attentionFindings")} id="dash-f">
              {s.open.length === 0 ? (
                <Empty text={t("dash.noOpenFindings")} />
              ) : (
                <ul className="list">
                  {s.open.slice(0, 6).map((c) => (
                    <li key={c.id}>
                      <Badge group="conflict.kind" value={c.kind} tone="crit" /> <b className="mono">{c.subject_key}</b>
                      <span className="muted att-why">{tm(c.explanation.msg, c.explanation.why)}</span>
                    </li>
                  ))}
                </ul>
              )}
              <button className="link" onClick={() => onNavigate("findings")}>{t("dash.reviewFindings")}</button>
            </Panel>
            <Panel title={t("dash.attentionDecisions")} id="dash-d">
              {affected.length === 0 ? (
                <Empty text={t("dash.noAffected")} />
              ) : (
                <ul className="list">
                  {affected.slice(0, 6).map((d) => (
                    <li key={d.id}>
                      <Badge group="decision.status" value={d.status} /> {d.title}
                      <span className="mono muted att-why">{d.subject_key}</span>
                    </li>
                  ))}
                </ul>
              )}
              <button className="link" onClick={() => onNavigate("affected")}>{t("dash.analyseImpact")}</button>
            </Panel>
          </div>
        </>
      )}
    </div>
  );
}
