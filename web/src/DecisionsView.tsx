import { useEffect, useState } from "react";
import type { Api } from "./api";

export interface Decision {
  id: string;
  kind: string;
  title: string;
  subject_key: string;
  outcome: string;
  status: string;
  decided_at: string;
  derivation_sha256: string;
}

interface Snap {
  num?: string | null;
  text?: string | null;
  unit?: string | null;
}

export interface Trace {
  decision: Decision & { rationale: string };
  derivation_reproducible: boolean;
  evidence: { claim_id?: string; upstream_decision?: string; predicate?: string; snapshot_at_decision: Snap; current_value?: Snap; current_status?: string; changed: boolean }[];
  invalidations: { reason: string; material: boolean; recorded_at: string; cause_kind: string }[];
  changed_since_decision: unknown[];
}

const show = (s?: Snap) => (s ? `${s.num ?? s.text ?? ""} ${s.unit ?? ""}`.trim() : "");

export function TraceView({ t, onAction }: { t: Trace; onAction: (action: "reconfirm" | "revoke", comment: string) => void }) {
  const [comment, setComment] = useState("");
  const d = t.decision;
  return (
    <div>
      <h3>
        {d.title} <span className={d.status === "valid" ? "badge" : "badge warn"}>{d.status}</span>
      </h3>
      <p>
        {d.kind} · outcome {d.outcome} · decided {d.decided_at} · derivation {d.derivation_sha256.slice(0, 16)}… (
        {t.derivation_reproducible ? "reproducible" : "NOT reproducible"})
      </p>
      <p>Rationale: {d.rationale}</p>
      <table>
        <thead>
          <tr>
            <th>Evidence</th>
            <th>At decision</th>
            <th>Now</th>
            <th>Changed</th>
          </tr>
        </thead>
        <tbody>
          {t.evidence.map((e, i) => (
            <tr key={i} className={e.changed ? "conflict" : undefined}>
              <td>{e.predicate ?? `decision ${e.upstream_decision?.slice(0, 8)}`}</td>
              <td>{show(e.snapshot_at_decision)}</td>
              <td>{e.current_value ? show(e.current_value) : e.current_status}</td>
              <td>{e.changed ? "yes" : "no"}</td>
            </tr>
          ))}
        </tbody>
      </table>
      <h4>Changes since decision</h4>
      <ul>
        {t.invalidations.map((i, k) => (
          <li key={k}>
            {i.recorded_at}: {i.reason} ({i.material ? "material" : "not material"})
          </li>
        ))}
      </ul>
      {d.status === "potentially_affected" && <p className="warn">Evidence changed. A reviewer must reconfirm or revoke this decision.</p>}
      {(d.status === "valid" || d.status === "potentially_affected") && (
        <div>
          <textarea aria-label="decision comment" value={comment} onChange={(e) => setComment(e.target.value)} />
          {d.status === "potentially_affected" && <button onClick={() => onAction("reconfirm", comment)}>Reconfirm with current evidence</button>}
          <button onClick={() => onAction("revoke", comment)}>Revoke</button>
        </div>
      )}
    </div>
  );
}

export function DecisionsView({ api }: { api: Api }) {
  const [items, setItems] = useState<Decision[]>([]);
  const [trace, setTrace] = useState<Trace | null>(null);
  const [report, setReport] = useState("");
  const [msg, setMsg] = useState("");
  const [f, setF] = useState({ title: "", kind: "supplier_part_release", subject_key: "", outcome: "approved", rationale: "", claims: "" });
  const reload = () => api.request<Decision[]>("GET", "/decisions").then(setItems).catch((e) => setMsg(String(e)));
  useEffect(() => {
    reload();
  }, []);
  async function open(id: string) {
    setReport("");
    setTrace(await api.request<Trace>("GET", `/decisions/${id}/trace`));
  }
  async function create(e: React.FormEvent) {
    e.preventDefault();
    try {
      const evidence = f.claims.split(/[\s,]+/).filter(Boolean).map((claim_id) => ({ claim_id }));
      await api.request("POST", "/decisions", { ...f, subject_type: "part", evidence, claims: undefined });
      setMsg("Decision recorded.");
      reload();
    } catch (x) {
      setMsg(String(x));
    }
  }
  async function act(action: "reconfirm" | "revoke", comment: string) {
    if (!trace) return;
    try {
      const d = await api.request<Decision>("POST", `/decisions/${trace.decision.id}/${action}`, { comment });
      await open(d.id);
      reload();
    } catch (x) {
      setMsg(String(x));
    }
  }
  return (
    <section>
      <h2>Decisions</h2>
      <form onSubmit={create} className="inline-form">
        <input aria-label="title" placeholder="title" value={f.title} onChange={(e) => setF({ ...f, title: e.target.value })} />
        <input aria-label="part" placeholder="part" value={f.subject_key} onChange={(e) => setF({ ...f, subject_key: e.target.value })} />
        <select aria-label="outcome" value={f.outcome} onChange={(e) => setF({ ...f, outcome: e.target.value })}>
          <option>approved</option>
          <option>rejected</option>
          <option>conditional</option>
        </select>
        <input aria-label="rationale" placeholder="rationale" value={f.rationale} onChange={(e) => setF({ ...f, rationale: e.target.value })} />
        <input aria-label="evidence claim ids" placeholder="evidence claim ids" value={f.claims} onChange={(e) => setF({ ...f, claims: e.target.value })} />
        <button>Record decision</button>
      </form>
      {msg && <p role="status">{msg}</p>}
      <table>
        <tbody>
          {items.map((d) => (
            <tr key={d.id} className="clickable" onClick={() => open(d.id)}>
              <td>{d.title}</td>
              <td>{d.subject_key}</td>
              <td>{d.outcome}</td>
              <td className={d.status === "valid" ? "" : "warn"}>{d.status}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {trace && (
        <>
          <TraceView t={trace} onAction={act} />
          <button onClick={async () => setReport(await api.text(`/decisions/${trace.decision.id}/report?format=markdown`))}>Verification report</button>
          {report && <pre className="report">{report}</pre>}
        </>
      )}
    </section>
  );
}
