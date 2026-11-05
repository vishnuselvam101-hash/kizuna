import { useEffect, useState } from "react";
import type { Api } from "./api";
import type { Artifact } from "./types";

export function ArtifactsView({ api }: { api: Api }) {
  const [items, setItems] = useState<Artifact[]>([]);
  const [msg, setMsg] = useState("");
  const [kind, setKind] = useState("spec_document");
  const reload = () => api.request<Artifact[]>("GET", "/artifacts").then(setItems).catch((e) => setMsg(String(e)));
  useEffect(() => {
    reload();
  }, []);
  async function upload(e: React.ChangeEvent<HTMLInputElement>) {
    const f = e.target.files?.[0];
    if (!f) return;
    const fd = new FormData();
    fd.append("kind", kind);
    fd.append("file", f);
    try {
      const r = await api.request<{ artifact: Artifact; duplicate: boolean }>("POST", "/artifacts", fd);
      setMsg(`${r.duplicate ? "Already stored (new capture recorded)" : "Stored"}: ${r.artifact.status}`);
      reload();
    } catch (x) {
      setMsg(String(x));
    }
  }
  async function extract(id: string) {
    try {
      const r = await api.request<{ claims_created: number; unmapped_labels: string[] }>("POST", `/artifacts/${id}/extract`, {});
      setMsg(`${r.claims_created} claims extracted (unreviewed). Unrecognised labels: ${r.unmapped_labels.join(", ") || "none"}`);
    } catch (x) {
      setMsg(String(x));
    }
  }
  async function verify(id: string) {
    const r = await api.request<{ intact: boolean }>("POST", `/artifacts/${id}/verify`, {});
    setMsg(r.intact ? "Stored bytes unchanged since capture (not a statement about truthfulness)." : "INTEGRITY FAILURE: stored bytes changed.");
  }
  return (
    <section>
      <h2>Documents</h2>
      <label>
        Kind{" "}
        <select value={kind} onChange={(e) => setKind(e.target.value)}>
          {["spec_document", "drawing", "purchase_order", "inspection_report", "spreadsheet", "other"].map((k) => (
            <option key={k}>{k}</option>
          ))}
        </select>
      </label>{" "}
      <input type="file" onChange={upload} aria-label="upload document" />
      {msg && <p role="status">{msg}</p>}
      <table>
        <thead>
          <tr>
            <th>Name</th>
            <th>Kind</th>
            <th>Type</th>
            <th>Status</th>
            <th>SHA-256</th>
            <th />
          </tr>
        </thead>
        <tbody>
          {items.map((a) => (
            <tr key={a.id}>
              <td>{a.display_name}</td>
              <td>{a.kind}</td>
              <td>{a.media_type}</td>
              <td className={a.status === "accepted" ? "" : "warn"}>{a.status}</td>
              <td>
                <code>{a.sha256.slice(0, 16)}…</code>
              </td>
              <td>
                <button onClick={() => verify(a.id)}>Verify integrity</button>{" "}
                {a.status === "accepted" && <button onClick={() => extract(a.id)}>Extract claims</button>}
              </td>
            </tr>
          ))}
        </tbody>
      </table>
    </section>
  );
}
