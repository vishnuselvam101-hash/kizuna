import { useEffect, useState } from "react";
import type { Api } from "./api";
import { claimValue, confidence, sourceLocation } from "./format";
import type { Claim } from "./types";

export interface Conflict {
  id: string;
  kind: string;
  subject_key: string;
  status: string;
  explanation: { why?: string; [k: string]: unknown };
  detected_at: string;
}

export interface ConflictDetail {
  conflict: Conflict;
  claim_a: Claim;
  claim_b: Claim | null;
  sources: { claim_id: string; display_name: string; location: Record<string, string | number> }[];
  reviews: { decision: string; comment: string | null; decided_at: string }[];
  requires_human_review: boolean;
  limitations: string;
  potentially_affected_decisions?: { id: string; title: string; status: string }[];
}

function ClaimCard({ c, label, d }: { c: Claim; label: string; d: ConflictDetail }) {
  const src = d.sources.find((s) => s.claim_id === c.id);
  return (
    <div className="card">
      <h4>{label}</h4>
      <p>
        <b>{c.predicate}</b> = {claimValue(c)} {c.revision && <>(rev {c.revision})</>}
      </p>
      <p>Source: {src ? `${src.display_name} · ${sourceLocation(src.location)}` : "manual entry"}</p>
      <p>
        {c.extraction_method} · confidence {confidence(c.confidence_bps)} · review: {c.review_status}
      </p>
    </div>
  );
}

export function ConflictDetailView({ d, onReview }: { d: ConflictDetail; onReview: (decision: string, comment: string) => void }) {
  const [comment, setComment] = useState("");
  const c = d.conflict;
  return (
    <div>
      <h3>
        {c.kind.replaceAll("_", " ")} — {c.subject_key} <span className="badge">{c.status}</span>
      </h3>
      <p>
        <b>Why:</b> {c.explanation.why ?? JSON.stringify(c.explanation)}
      </p>
      {d.requires_human_review && <p className="warn">Human review required.</p>}
      <div className="cards">
        <ClaimCard c={d.claim_a} label="Evidence A" d={d} />
        {d.claim_b && <ClaimCard c={d.claim_b} label="Evidence B" d={d} />}
      </div>
      {d.potentially_affected_decisions && d.potentially_affected_decisions.length > 0 && (
        <p>
          Potentially affected decisions: {d.potentially_affected_decisions.map((x) => `${x.title} (${x.status})`).join(", ")}
        </p>
      )}
      <p className="muted">Limitations: {d.limitations}</p>
      {d.reviews.map((r, i) => (
        <p key={i}>
          Review: {r.decision} — {r.comment} ({r.decided_at})
        </p>
      ))}
      {d.requires_human_review && (
        <div>
          <textarea value={comment} onChange={(e) => setComment(e.target.value)} placeholder="Reason (required to approve or reject)" aria-label="review comment" />
          <div>
            <button onClick={() => onReview("approve", comment)}>Approve (waive)</button>
            <button onClick={() => onReview("reject", comment)}>Reject evidence</button>
            <button onClick={() => onReview("request_evidence", comment)}>Request more evidence</button>
          </div>
        </div>
      )}
    </div>
  );
}

export function ConflictsView({ api }: { api: Api }) {
  const [items, setItems] = useState<Conflict[]>([]);
  const [detail, setDetail] = useState<ConflictDetail | null>(null);
  const [msg, setMsg] = useState("");
  const reload = () => api.request<Conflict[]>("GET", "/conflicts?status=open").then(setItems).catch((e) => setMsg(String(e)));
  useEffect(() => {
    reload();
  }, []);
  async function run() {
    const r = await api.request<{ findings_new: number; marked_obsolete: number }>("POST", "/verification/run", {});
    setMsg(`${r.findings_new} new findings, ${r.marked_obsolete} no longer apply.`);
    reload();
  }
  async function open(id: string) {
    setDetail(await api.request<ConflictDetail>("GET", `/conflicts/${id}`));
  }
  async function review(decision: string, comment: string) {
    if (!detail) return;
    try {
      await api.request("POST", `/conflicts/${detail.conflict.id}/review`, { decision, comment });
      await open(detail.conflict.id);
      reload();
    } catch (x) {
      setMsg(String(x));
    }
  }
  return (
    <section>
      <h2>Findings</h2>
      <button onClick={run}>Run verification</button>
      {msg && <p role="status">{msg}</p>}
      <table>
        <thead>
          <tr>
            <th>Kind</th>
            <th>Part</th>
            <th>Why</th>
            <th>Status</th>
          </tr>
        </thead>
        <tbody>
          {items.map((c) => (
            <tr key={c.id} onClick={() => open(c.id)} className="clickable">
              <td>{c.kind}</td>
              <td>{c.subject_key}</td>
              <td>{c.explanation.why}</td>
              <td>{c.status}</td>
            </tr>
          ))}
        </tbody>
      </table>
      {detail && <ConflictDetailView d={detail} onReview={review} />}
    </section>
  );
}

export function CompareView({ api }: { api: Api }) {
  const [q, setQ] = useState({ key: "", from: "", to: "" });
  const [rows, setRows] = useState<{ predicate: string; from: string | null; to: string | null; change: string }[] | null>(null);
  async function go(e: React.FormEvent) {
    e.preventDefault();
    const r = await api.request<{ differences: typeof rows }>(
      "GET",
      `/compare/part/${encodeURIComponent(q.key)}?from=${encodeURIComponent(q.from)}&to=${encodeURIComponent(q.to)}`,
    );
    setRows(r.differences);
  }
  return (
    <section>
      <h2>Revision comparison</h2>
      <form onSubmit={go}>
        <input aria-label="part" placeholder="part" value={q.key} onChange={(e) => setQ({ ...q, key: e.target.value })} />
        <input aria-label="from revision" placeholder="from" value={q.from} onChange={(e) => setQ({ ...q, from: e.target.value })} />
        <input aria-label="to revision" placeholder="to" value={q.to} onChange={(e) => setQ({ ...q, to: e.target.value })} />
        <button>Compare</button>
      </form>
      {rows && (
        <table>
          <tbody>
            {rows.map((r) => (
              <tr key={r.predicate}>
                <td>{r.predicate}</td>
                <td>{r.from ?? "—"}</td>
                <td>{r.to ?? "—"}</td>
                <td>{r.change}</td>
              </tr>
            ))}
          </tbody>
        </table>
      )}
    </section>
  );
}
