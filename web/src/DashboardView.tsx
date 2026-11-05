import { useMemo } from "react";
import { GraphScene, type GNode, type Graph } from "./graph";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Artifact, ChainStatus, Claim, Conflict, Decision } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";
import type { View } from "./nav";

const MAX_CLAIMS = 300;

export function orgGraph(s: Summary): Graph {
  const nodes: GNode[] = [];
  const edges: Graph["edges"] = [];
  const seen = new Set<string>();
  const add = (n: GNode) => !seen.has(n.id) && (seen.add(n.id), nodes.push(n));
  s.artifacts.forEach((a) => add({ id: `doc:${a.id}`, kind: "document", label: a.display_name, tone: a.status === "accepted" ? "base" : "warn" }));
  const subjectTone = new Map<string, GNode["tone"]>();
  s.open.forEach((c) => subjectTone.set(c.subject_key, "crit"));
  s.claims
    .filter((c) => !c.superseded_at)
    .slice(0, MAX_CLAIMS)
    .forEach((c) => {
      const cid = `claim:${c.id}`;
      const sid = `subj:${c.subject_type}:${c.subject_key}`;
      add({ id: cid, kind: "claim", label: `${c.subject_key} ${c.predicate}`, tone: c.review_status === "rejected" ? "crit" : c.review_status === "accepted" ? "ok" : "base" });
      add({ id: sid, kind: "subject", label: c.subject_key, tone: subjectTone.get(c.subject_key) ?? "info" });
      if (c.artifact_id && seen.has(`doc:${c.artifact_id}`)) edges.push([`doc:${c.artifact_id}`, cid]);
      edges.push([cid, sid]);
    });
  const subj = (key: string) => nodes.find((n) => n.kind === "subject" && n.label === key)?.id;
  s.decisions.forEach((d) => {
    add({ id: `dec:${d.id}`, kind: "decision", label: d.title, tone: d.status === "valid" ? "ok" : d.status === "potentially_affected" ? "crit" : "base" });
    const sid = subj(d.subject_key);
    if (sid) edges.push([sid, `dec:${d.id}`]);
  });
  s.open.forEach((c) => {
    add({ id: `fnd:${c.id}`, kind: "finding", label: c.subject_key, tone: "crit" });
    const sid = subj(c.subject_key);
    if (sid) edges.push([sid, `fnd:${c.id}`]);
  });
  return { nodes, edges };
}

const PICK: Record<string, View> = { doc: "documents", claim: "records", subj: "records", dec: "decisions", fnd: "findings" };

interface Summary {
  claims: Claim[];
  artifacts: Artifact[];
  open: Conflict[];
  needsEvidence: Conflict[];
  decisions: Decision[];
  chain: ChainStatus | null;
}

export function DashboardView({ onNavigate }: { onNavigate: (v: View) => void }) {
  const { api, me, can } = useSession();
  const { t, fmtNum } = useI18n();
  const { data, error, loading } = useLoad<Summary>(async () => {
    const [claims, artifacts, open, needsEvidence, decisions, chain] = await Promise.all([
      api.request<Claim[]>("GET", "/claims"),
      api.request<Artifact[]>("GET", "/artifacts"),
      api.request<Conflict[]>("GET", "/conflicts?status=open"),
      api.request<Conflict[]>("GET", "/conflicts?status=needs_evidence"),
      api.request<Decision[]>("GET", "/decisions"),
      can("audit_verify") ? api.request<ChainStatus>("GET", "/audit/verify") : Promise.resolve(null),
    ]);
    return { claims, artifacts, open, needsEvidence, decisions, chain };
  }, []);

  const s = data;
  const graph = useMemo(() => (s ? orgGraph(s) : null), [s]);
  const count = (k: GNode["kind"]) => graph?.nodes.filter((n) => n.kind === k).length ?? 0;
  const affected = s?.decisions.filter((d) => d.status === "potentially_affected") ?? [];
  const quarantined = s?.artifacts.filter((a) => a.status !== "accepted").length ?? 0;
  const kpis = s
    ? [
        { k: "dash.kpi.documents", v: s.artifacts.length, sub: t("dash.kpi.notAccepted", { n: fmtNum(quarantined) }), go: "documents" as View, tone: quarantined ? "warn" : "" },
        { k: "dash.kpi.openFindings", v: s.open.length, sub: t("dash.kpi.needsEvidence", { n: fmtNum(s.needsEvidence.length) }), go: "findings" as View, tone: s.open.length ? "crit" : "" },
        { k: "dash.kpi.decisions", v: s.decisions.filter((d) => d.status === "valid").length, sub: t("dash.kpi.validOfTotal", { n: fmtNum(s.decisions.length) }), go: "decisions" as View, tone: "" },
        { k: "dash.kpi.affected", v: affected.length, sub: t("dash.kpi.affectedSub"), go: "affected" as View, tone: affected.length ? "crit" : "" },
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
                  <span className="kpi-sub">{k.sub}</span>
                </button>
              </li>
            ))}
            {s.chain && (
              <li className={`kpi ${s.chain.intact ? "" : "crit"}`}>
                <button className="kpi-btn" onClick={() => onNavigate("reports")}>
                  <span className="kpi-label">{t("dash.kpi.auditChain")}</span>
                  <span className="kpi-value">{s.chain.intact ? t("audit.intact") : t("audit.broken")}</span>
                  <span className="kpi-sub">{t("audit.checked", { n: fmtNum(s.chain.events_checked) })}</span>
                </button>
              </li>
            )}
          </ul>
          {graph && graph.nodes.length > 0 && (
            <Panel title={t("dash.graph")} id="dash-g">
              <p className="muted">
                {t("dash.graphLead")} {s.claims.length > MAX_CLAIMS && t("dash.graphCapped", { n: MAX_CLAIMS })}
              </p>
              <GraphScene
                g={graph}
                label={t("dash.graphLabel", { docs: count("document"), claims: count("claim"), subjects: count("subject"), decisions: count("decision"), findings: count("finding") })}
                onPick={(id) => onNavigate(PICK[id.split(":")[0]] ?? "dashboard")}
              />
            </Panel>
          )}
          <div className="grid2">
            <Panel title={t("dash.attentionFindings")} id="dash-f">
              {s.open.length === 0 ? (
                <Empty text={t("dash.noOpenFindings")} />
              ) : (
                <ul className="list">
                  {s.open.slice(0, 6).map((c) => (
                    <li key={c.id}>
                      <Badge group="conflict.kind" value={c.kind} tone="crit" /> <b className="mono">{c.subject_key}</b>
                      <span className="muted"> — {c.explanation.why}</span>
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
                      <Badge group="decision.status" value={d.status} /> {d.title} <span className="mono muted">{d.subject_key}</span>
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
