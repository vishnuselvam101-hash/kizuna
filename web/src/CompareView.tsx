import { useState } from "react";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Difference } from "./types";
import { Badge, Empty, Notice, PageTitle, Panel } from "./ui";

export function CompareView() {
  const { api } = useSession();
  const { t } = useI18n();
  const [q, setQ] = useState({ type: "part", key: "", from: "", to: "" });
  const [rows, setRows] = useState<Difference[] | null>(null);
  const [err, setErr] = useState<unknown>(null);
  async function go(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    try {
      const r = await api.request<{ differences: Difference[] }>(
        "GET",
        `/compare/${q.type}/${encodeURIComponent(q.key)}?from=${encodeURIComponent(q.from)}&to=${encodeURIComponent(q.to)}`,
      );
      setRows(r.differences);
    } catch (x) {
      setErr(x);
    }
  }
  const field = (k: keyof typeof q, label: string) => (
    <label>
      {label}
      <input required value={q[k]} onChange={(e) => setQ({ ...q, [k]: e.target.value })} />
    </label>
  );
  return (
    <div>
      <PageTitle title={t("nav.revisions")} lead={t("compare.lead")} />
      <Panel title={t("compare.query")} id="cmp-q">
        <form onSubmit={go} className="form-row">
          <label>
            {t("compare.type")}
            <select value={q.type} onChange={(e) => setQ({ ...q, type: e.target.value })}>
              <option value="part">{t("records.components")}</option>
              <option value="supplier">{t("records.suppliers")}</option>
            </select>
          </label>
          {field("key", t("compare.key"))}
          {field("from", t("compare.from"))}
          {field("to", t("compare.to"))}
          <button className="primary">{t("compare.run")}</button>
        </form>
      </Panel>
      <Notice error={err} />
      {rows && (
        <Panel title={t("compare.result", { key: q.key, from: q.from, to: q.to })} id="cmp-r">
          {rows.length === 0 ? (
            <Empty text={t("compare.noDiff")} />
          ) : (
            <div className="table-wrap" tabIndex={0} role="region" aria-label={t("compare.query")}>
              <table>
                <thead>
                  <tr>
                    <th scope="col">{t("evidence.predicate")}</th>
                    <th scope="col">{t("compare.fromValue", { rev: q.from })}</th>
                    <th scope="col">{t("compare.toValue", { rev: q.to })}</th>
                    <th scope="col">{t("compare.change")}</th>
                  </tr>
                </thead>
                <tbody>
                  {rows.map((r) => (
                    <tr key={r.predicate}>
                      <td className="mono">{r.predicate}</td>
                      <td className="mono">{r.from ?? "—"}</td>
                      <td className="mono">{r.to ?? "—"}</td>
                      <td>
                        <Badge group="change" value={r.change} />
                      </td>
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          )}
        </Panel>
      )}
    </div>
  );
}
