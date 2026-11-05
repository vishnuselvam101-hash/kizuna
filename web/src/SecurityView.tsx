import { useState } from "react";
import { DataTable } from "./DataTable";
import { DetectionSettingsPanel } from "./DetectionSettingsPanel";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";

export interface Incident {
  id: string;
  rule: string;
  severity: string;
  subject: string;
  summary: string;
  status: string;
  last_seen: string;
}

export interface IncidentAction {
  id: string;
  kind: string;
  target: string;
  policy: string;
  status: string;
  detail: string | null;
}

interface Summary {
  open_incidents: number;
  actions_awaiting_approval: number;
  events_24h: Record<string, { severity: string; count: number }>;
  active_ip_blocks: { ip: string; until: number }[];
}

const REVERSIBLE = new Set(["lock_account", "block_ip", "restrict_uploads", "suspend_extraction"]);
const sevTone = (s: string) => (s === "critical" || s === "high" ? "crit" : s === "medium" ? "warn" : "info");

export function ActionRow({ a, canManage, onDecide }: { a: IncidentAction; canManage: boolean; onDecide: (id: string, verb: string) => void }) {
  const { t, tv } = useI18n();
  return (
    <tr>
      <td>{tv("sec.action", a.kind)}</td>
      <td>
        <code>{a.target}</code>
      </td>
      <td>{a.policy === "requires_approval" ? t("sec.needs_approval") : t("sec.automatic")}</td>
      <td>
        <Badge group="sec.astatus" value={a.status} />
      </td>
      <td>{a.detail}</td>
      <td>
        {canManage && a.status === "proposed" && (
          <>
            <button onClick={() => onDecide(a.id, "approve")}>{t("sec.approve")}</button>{" "}
            <button onClick={() => onDecide(a.id, "reject")}>{t("sec.reject")}</button>
          </>
        )}
        {canManage && a.status === "executed" && REVERSIBLE.has(a.kind) && <button onClick={() => onDecide(a.id, "revert")}>{t("sec.revert")}</button>}
      </td>
    </tr>
  );
}

export function SecurityView() {
  const { api, can } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const summary = useLoad(() => api.request<Summary>("GET", "/security/summary"), []);
  const list = useLoad(() => api.request<Incident[]>("GET", "/security/incidents"), []);
  const [sel, setSel] = useState<{ incident: Incident; actions: IncidentAction[] } | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [comment, setComment] = useState("");
  const manage = can("incident_manage");

  const open = async (id: string) => {
    setErr(null);
    try {
      setSel(await api.request("GET", `/security/incidents/${id}`));
    } catch (x) {
      setErr(x);
    }
  };
  async function decide(actionId: string, verb: string) {
    if (!sel) return;
    try {
      await api.request("POST", `/security/incidents/${sel.incident.id}/actions/${actionId}/${verb}`, { comment });
      await open(sel.incident.id);
      list.reload();
      summary.reload();
    } catch (x) {
      setErr(x);
    }
  }
  async function resolve(status: "resolved" | "false_positive") {
    if (!sel) return;
    try {
      await api.request("POST", `/security/incidents/${sel.incident.id}/resolve`, { status, comment });
      await open(sel.incident.id);
      list.reload();
    } catch (x) {
      setErr(x);
    }
  }
  const s = summary.data;
  return (
    <>
      <PageTitle title={t("nav.security")} lead={t("sec.lead")} />
      <Notice error={err ?? summary.error ?? list.error} />
      <Loading on={summary.loading || list.loading} />
      {s && (
        <ul className="kpis" aria-label={t("sec.lead")}>
          {[
            [s.open_incidents, t("sec.open_incidents"), s.open_incidents ? "crit" : ""],
            [s.actions_awaiting_approval, t("sec.awaiting"), s.actions_awaiting_approval ? "warn" : ""],
            [s.active_ip_blocks.length, t("sec.blocks"), ""],
          ].map(([n, label, tone]) => (
            <li key={String(label)} className={`kpi ${tone}`}>
              <span className="kpi-btn">
                <span className="kpi-label">{label}</span>
                <span className="kpi-value num">{n}</span>
              </span>
            </li>
          ))}
        </ul>
      )}
      <Panel title={t("sec.incidents")}>
        <DataTable
          rows={list.data ?? []}
          rowKey={(i) => i.id}
          label={t("sec.incidents")}
          onActivate={(i) => open(i.id)}
          selectedKey={sel?.incident.id ?? null}
          cols={[
            { key: "sev", label: t("sec.severity"), value: (i) => i.severity, render: (i) => <Badge group="sec.sev" value={i.severity} tone={sevTone(i.severity)} /> },
            { key: "rule", label: t("sec.rule"), value: (i) => i.rule, render: (i) => tv("sec.rule", i.rule), className: "nowrap" },
            { key: "subject", label: t("sec.subject"), value: (i) => i.subject, render: (i) => <code>{i.subject}</code> },
            { key: "summary", label: t("sec.summary"), render: (i) => i.summary },
            { key: "status", label: t("sec.status"), value: (i) => i.status, render: (i) => <Badge group="sec.istatus" value={i.status} /> },
            { key: "seen", label: t("sec.last_seen"), value: (i) => i.last_seen, render: (i) => fmtDate(i.last_seen), className: "num" },
          ]}
        />
      </Panel>
      {sel && (
        <Panel title={tv("sec.rule", sel.incident.rule)}>
          <p>{sel.incident.summary}</p>
          <table className="table">
            <tbody>
              {sel.actions.map((a) => (
                <ActionRow key={a.id} a={a} canManage={manage} onDecide={decide} />
              ))}
            </tbody>
          </table>
          {manage && (sel.incident.status === "open" || sel.incident.status === "contained") && (
            <div>
              <label className="stack">
                {t("sec.comment")}
                <textarea aria-label={t("sec.comment")} value={comment} onChange={(e) => setComment(e.target.value)} />
              </label>
              <button onClick={() => resolve("resolved")}>{t("sec.resolve")}</button>{" "}
              <button onClick={() => resolve("false_positive")}>{t("sec.false_positive")}</button>
            </div>
          )}
          <p className="muted">{t("sec.defensive_only")}</p>
        </Panel>
      )}
      {can("user_manage") && <DetectionSettingsPanel />}
    </>
  );
}
