import { useEffect, useState } from "react";
import type { Api } from "./api";

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

export function ActionRow({ a, onDecide }: { a: IncidentAction; onDecide: (id: string, verb: string) => void }) {
  return (
    <tr>
      <td>{a.kind}</td>
      <td>{a.target}</td>
      <td>{a.policy === "requires_approval" ? "needs approval" : "automatic"}</td>
      <td>{a.status}</td>
      <td>{a.detail}</td>
      <td>
        {a.status === "proposed" && (
          <>
            <button onClick={() => onDecide(a.id, "approve")}>Approve</button>
            <button onClick={() => onDecide(a.id, "reject")}>Reject</button>
          </>
        )}
        {a.status === "executed" && a.kind !== "notify_admins" && a.kind !== "revoke_user_sessions" && (
          <button onClick={() => onDecide(a.id, "revert")}>Revert</button>
        )}
      </td>
    </tr>
  );
}

export function SecurityView({ api }: { api: Api }) {
  const [summary, setSummary] = useState<{ open_incidents: number; actions_awaiting_approval: number; events_24h: Record<string, { severity: string; count: number }> } | null>(null);
  const [items, setItems] = useState<Incident[]>([]);
  const [sel, setSel] = useState<{ incident: Incident; actions: IncidentAction[] } | null>(null);
  const [msg, setMsg] = useState("");
  const reload = async () => {
    try {
      setSummary(await api.request("GET", "/security/summary"));
      setItems(await api.request("GET", "/security/incidents"));
    } catch (x) {
      setMsg(String(x));
    }
  };
  useEffect(() => {
    reload();
  }, []);
  const open = async (id: string) => setSel(await api.request("GET", `/security/incidents/${id}`));
  async function decide(actionId: string, verb: string) {
    if (!sel) return;
    try {
      await api.request("POST", `/security/incidents/${sel.incident.id}/actions/${actionId}/${verb}`, { comment: "via dashboard" });
      await open(sel.incident.id);
      reload();
    } catch (x) {
      setMsg(String(x));
    }
  }
  return (
    <section>
      <h2>Security</h2>
      {msg && <p role="alert">{msg}</p>}
      {summary && (
        <p>
          Open incidents: <b>{summary.open_incidents}</b> · actions awaiting approval: <b>{summary.actions_awaiting_approval}</b> · event kinds (24 h):{" "}
          {Object.entries(summary.events_24h)
            .map(([k, v]) => `${k} ${v.count}`)
            .join(", ") || "none"}
        </p>
      )}
      <table>
        <tbody>
          {items.map((i) => (
            <tr key={i.id} className="clickable" onClick={() => open(i.id)}>
              <td className={i.severity === "critical" || i.severity === "high" ? "warn" : ""}>{i.severity}</td>
              <td>{i.rule}</td>
              <td>{i.subject}</td>
              <td>{i.summary}</td>
              <td>{i.status}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {sel && (
        <div>
          <h3>{sel.incident.rule}</h3>
          <table>
            <tbody>
              {sel.actions.map((a) => (
                <ActionRow key={a.id} a={a} onDecide={decide} />
              ))}
            </tbody>
          </table>
        </div>
      )}
    </section>
  );
}
