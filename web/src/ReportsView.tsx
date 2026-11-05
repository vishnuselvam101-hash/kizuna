import { useState } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { AuditEvent, ChainStatus, Decision, OrgReport } from "./types";
import { Empty, Loading, Notice, PageTitle, Panel, download, short, useLoad } from "./ui";

function DecisionReport() {
  const { api } = useSession();
  const { t, tv } = useI18n();
  const decisions = useLoad(() => api.request<Decision[]>("GET", "/decisions"), []);
  const [id, setId] = useState("");
  const [format, setFormat] = useState<"markdown" | "json">("markdown");
  const [report, setReport] = useState<{ text: string; format: string; id: string } | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  async function generate(e: React.FormEvent) {
    e.preventDefault();
    if (!id) return;
    setErr(null);
    setBusy(true);
    try {
      const text =
        format === "markdown"
          ? await api.text(`/decisions/${id}/report?format=markdown`)
          : JSON.stringify(await api.request("GET", `/decisions/${id}/report?format=json`), null, 2);
      setReport({ text, format, id });
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title={t("reports.decision")} id="rep-d">
      <p className="muted">{t("reports.decisionLead")}</p>
      <form onSubmit={generate} className="form-row">
        <label>
          {t("reports.pickDecision")}
          <select value={id} onChange={(e) => setId(e.target.value)} required>
            <option value="">{t("reports.choose")}</option>
            {(decisions.data ?? []).map((d) => (
              <option key={d.id} value={d.id}>
                {d.title} · {d.subject_key} · {tv("decision.status", d.status)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("reports.format")}
          <select value={format} onChange={(e) => setFormat(e.target.value as "markdown" | "json")}>
            <option value="markdown">{t("reports.markdown")}</option>
            <option value="json">{t("reports.json")}</option>
          </select>
        </label>
        <button className="primary" disabled={!id || busy}>
          {t("reports.generate")}
        </button>
      </form>
      <Notice error={err ?? decisions.error} />
      {report && (
        <>
          <div className="actions">
            <button
              onClick={() =>
                download(
                  `kizuna-decision-${short(report.id)}.${report.format === "json" ? "json" : "md"}`,
                  report.text,
                  report.format === "json" ? "application/json" : "text/markdown",
                )
              }
            >
              {t("reports.download")}
            </button>
          </div>
          <pre className="report" tabIndex={0} aria-label={t("reports.preview")}>
            {report.text}
          </pre>
        </>
      )}
    </Panel>
  );
}

function OrgReportPanel() {
  const { api } = useSession();
  const { t, fmtNum } = useI18n();
  const [q, setQ] = useState({ scope: "open_findings", from: "", to: "" });
  const [report, setReport] = useState<OrgReport | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  async function generate(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    setBusy(true);
    try {
      // Plain calendar dates, both inclusive; the server applies the tenant's timezone.
      const day = (d: string) => (d ? d : undefined);
      setReport(
        await api.request<OrgReport>("POST", "/reports", { scope: q.scope, from_date: day(q.from), to_date: day(q.to) }),
      );
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title={t("reports.org")} id="rep-o">
      <p className="muted">{t("reports.orgLead")}</p>
      <form onSubmit={generate} className="form-row">
        <label>
          {t("reports.scope")}
          <select value={q.scope} onChange={(e) => setQ({ ...q, scope: e.target.value })}>
            <option value="open_findings">{t("reports.scopeOpen")}</option>
            <option value="all">{t("reports.scopeAll")}</option>
          </select>
        </label>
        <label>
          {t("reports.from")}
          <input type="date" value={q.from} onChange={(e) => setQ({ ...q, from: e.target.value })} />
        </label>
        <label>
          {t("reports.to")}
          <input type="date" value={q.to} onChange={(e) => setQ({ ...q, to: e.target.value })} />
        </label>
        <button className="primary" disabled={busy}>
          {t("reports.generateOrg")}
        </button>
      </form>
      <Notice error={err} />
      {report && (
        <>
          <p className={report.audit_chain.intact ? "callout ok" : "callout crit"} role="status">
            {t("reports.orgSummary", {
              findings: fmtNum(report.findings.length),
              decisions: fmtNum(report.decisions.length),
              chain: report.audit_chain.intact ? t("audit.intact") : t("audit.broken"),
            })}
          </p>
          <dl className="kv">
            <dt>{t("reports.sha")}</dt>
            <dd>
              <code>{report.report_sha256}</code>
            </dd>
            <dt>{t("reports.signature")}</dt>
            <dd>
              {report.signature
                ? t("reports.signed", { alg: report.signature.algorithm, key: report.signature.public_key.slice(0, 16) })
                : t("reports.unsigned")}
            </dd>
          </dl>
          <p className="muted">{report.statement}</p>
          <div className="actions">
            <button onClick={() => download("kizuna-org-report.json", JSON.stringify(report, null, 2), "application/json")}>
              {t("reports.downloadOrg")}
            </button>
          </div>
        </>
      )}
    </Panel>
  );
}

function AuditTrail() {
  const { api, can } = useSession();
  const { t, fmtDate, fmtNum } = useI18n();
  const [events, setEvents] = useState<AuditEvent[]>([]);
  const page = useLoad(async () => {
    const r = await api.request<AuditEvent[]>("GET", "/audit/events?limit=200");
    setEvents(r);
    return r;
  }, []);
  const [chain, setChain] = useState<ChainStatus | null>(null);
  const [err, setErr] = useState<unknown>(null);
  async function more() {
    const last = events[events.length - 1]?.seq ?? 0;
    try {
      const r = await api.request<AuditEvent[]>("GET", `/audit/events?after_seq=${last}&limit=200`);
      setEvents((e) => [...e, ...r]);
    } catch (x) {
      setErr(x);
    }
  }
  async function verify() {
    setErr(null);
    try {
      setChain(await api.request<ChainStatus>("GET", "/audit/verify"));
    } catch (x) {
      setErr(x);
    }
  }
  return (
    <Panel
      title={t("reports.audit")}
      id="rep-a"
      actions={
        <>
          {can("audit_verify") && <button onClick={verify}>{t("reports.verifyChain")}</button>}
          <button disabled={!events.length} onClick={() => download("kizuna-audit-events.json", JSON.stringify(events, null, 2), "application/json")}>
            {t("reports.export")}
          </button>
        </>
      }
    >
      <Notice error={err ?? page.error} />
      {chain && (
        <p className={chain.intact ? "callout ok" : "callout crit"} role="status">
          {chain.intact
            ? t("reports.chainOk", { n: fmtNum(chain.events_checked), seq: chain.head_seq, hash: chain.head_hash.slice(0, 16) })
            : t("reports.chainBroken", { problem: chain.problem ?? "" })}
        </p>
      )}
      <Loading on={page.loading} />
      {page.data && events.length === 0 && <Empty text={t("reports.noEvents")} />}
      {events.length > 0 && (
        <DataTable
          label={t("reports.audit")}
          rows={events}
          rowKey={(e) => String(e.seq)}
          cols={[
            { key: "seq", label: t("reports.seq"), value: (e) => e.seq, render: (e) => e.seq, className: "num" },
            { key: "at", label: t("reports.at"), value: (e) => e.occurred_at, render: (e) => fmtDate(e.occurred_at), className: "num" },
            { key: "actor", label: t("reports.actor"), value: (e) => e.actor_kind, render: (e) => `${e.actor_kind}${e.actor_user_id ? ` ${short(e.actor_user_id)}` : ""}` },
            { key: "action", label: t("reports.action"), value: (e) => e.action, render: (e) => <span className="mono">{e.action}</span> },
            { key: "target", label: t("reports.target"), value: (e) => e.target_type, render: (e) => `${e.target_type}${e.target_id ? ` ${short(e.target_id)}` : ""}` },
            { key: "hash", label: t("reports.hash"), render: (e) => <code title={e.hash}>{e.hash.slice(0, 10)}…</code> },
          ]}
        />
      )}
      {events.length > 0 && events.length % 200 === 0 && (
        <button className="link" onClick={more}>
          {t("reports.more")}
        </button>
      )}
    </Panel>
  );
}

export function ReportsView() {
  const { can } = useSession();
  const { t } = useI18n();
  return (
    <div>
      <PageTitle title={t("nav.reports")} lead={t("reports.lead")} />
      <DecisionReport />
      {can("audit_read") ? (
        <>
          <OrgReportPanel />
          <AuditTrail />
        </>
      ) : (
        <p className="muted">{t("reports.auditNoAccess")}</p>
      )}
    </div>
  );
}
