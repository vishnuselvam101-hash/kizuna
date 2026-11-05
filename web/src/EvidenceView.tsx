import { useState } from "react";
import type { Api } from "./api";
import { claimValue, confidence, sourceLocation } from "./format";
import type { PredicateEvidence } from "./types";

export function EvidenceTable({ groups }: { groups: PredicateEvidence[] }) {
  if (!groups.length) return <p>No claims recorded for this subject.</p>;
  return (
    <table>
      <thead>
        <tr>
          <th>Predicate</th>
          <th>Value</th>
          <th>Revision</th>
          <th>Source</th>
          <th>Method / confidence</th>
          <th>Review</th>
        </tr>
      </thead>
      <tbody>
        {groups.flatMap((g) =>
          g.current.map((c, i) => (
            <tr key={c.id} className={g.disagreement ? "conflict" : undefined}>
              <td>
                {i === 0 ? g.predicate : ""}
                {i === 0 && g.disagreement && <span className="badge warn"> sources disagree</span>}
                {i === 0 && g.superseded_count > 0 && <span className="badge"> {g.superseded_count} superseded</span>}
              </td>
              <td>{claimValue(c)}</td>
              <td>{c.revision ?? "—"}</td>
              <td>
                {c.artifact_id ? `artifact ${c.artifact_id.slice(0, 8)}…` : "manual entry"} · {sourceLocation(c.source_location)}
              </td>
              <td>
                {c.extraction_method} · {confidence(c.confidence_bps)}
              </td>
              <td>{c.review_status}</td>
            </tr>
          )),
        )}
      </tbody>
    </table>
  );
}

export function EvidenceView({ api }: { api: Api }) {
  const [type, setType] = useState("part");
  const [key, setKey] = useState("");
  const [groups, setGroups] = useState<PredicateEvidence[] | null>(null);
  const [err, setErr] = useState("");
  async function load(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    try {
      const r = await api.request<{ predicates: PredicateEvidence[] }>(
        "GET",
        `/evidence/${encodeURIComponent(type)}/${encodeURIComponent(key)}`,
      );
      setGroups(r.predicates);
    } catch (x) {
      setErr(String(x));
    }
  }
  return (
    <section>
      <h2>Evidence explorer</h2>
      <form onSubmit={load}>
        <select value={type} onChange={(e) => setType(e.target.value)} aria-label="subject type">
          {["part", "supplier", "purchase_order", "drawing", "inspection_lot"].map((t) => (
            <option key={t}>{t}</option>
          ))}
        </select>
        <input value={key} onChange={(e) => setKey(e.target.value)} placeholder="identifier, e.g. BRK-100" aria-label="subject key" />
        <button>Show evidence</button>
      </form>
      {err && <p role="alert">{err}</p>}
      {groups && <EvidenceTable groups={groups} />}
    </section>
  );
}
