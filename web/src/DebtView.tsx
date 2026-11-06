import { useState } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad, type Tone } from "./ui";

export interface DebtRow {
  kind: string;
  target: { type: string; id?: string; name?: string };
  label: string;
  detail: Record<string, unknown> | null;
  since: string;
  age_days: number;
  blocks: number;
  decisions: { id: string; title: string; kind: string; part: string; supplier: string }[];
  parts: string[];
  suppliers: string[];
}
export interface DebtItems {
  version: string;
  as_of: string;
  kinds: string[];
  by_kind: Record<string, number>;
  rows: DebtRow[];
  total_rows: number;
  limitations: string[];
}
interface Ledger {
  totals: { decisions: number; with_debt: number };
}

// Hard evidence gaps use the critical tone; everything else needs a person to look (amber), never red by default.
const kindTone = (k: string): Tone => (k === "missing_required" || k === "rejected" || k === "expired" ? "crit" : "warn");
const rowKey = (r: DebtRow) => `${r.kind}:${r.target.id ?? r.target.name}:${r.parts.join(",")}`;

export function DebtView() {
  const { api } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const [kind, setKind] = useState("");
  const [asOf, setAsOf] = useState("");
  const [sel, setSel] = useState<DebtRow | null>(null);
  const qs = `?limit=500${kind ? `&kind=${kind}` : ""}${asOf ? `&as_of=${asOf}` : ""}`;
  const items = useLoad(() => api.request<DebtItems>("GET", `/evidence-debt/items${qs}`), [qs]);
  const ledger = useLoad(() => api.request<Ledger>("GET", `/evidence-debt${asOf ? `?as_of=${asOf}` : ""}`), [asOf]);
  const label = (r: DebtRow) => (r.target.type === "requirement" ? tv("debt.req", r.label) : r.label || r.target.id || "");
  return (
    <div>
      <PageTitle title={t("nav.debt")} lead={t("debt.lead")} />
      <div className="toolbar">
        <label>
          {t("debt.kind")}
          <select value={kind} onChange={(e) => (setKind(e.target.value), setSel(null))}>
            <option value="">{t("debt.allKinds")}</option>
            {(items.data?.kinds ?? []).map((k) => (
              <option key={k} value={k}>
                {tv("debt.kind", k)}
              </option>
            ))}
          </select>
        </label>
        <label>
          {t("debt.asOf")}
          <input type="date" value={asOf} onChange={(e) => (setAsOf(e.target.value), setSel(null))} />
        </label>
      </div>
      <Notice error={items.error ?? ledger.error} />
      {ledger.data && <p className="muted">{t("debt.summary", { decisions: ledger.data.totals.decisions, debt: ledger.data.totals.with_debt })}</p>}
      <div className="grid2 wide-right">
        <Panel title={t("debt.items")} id="debt-list">
          <Loading on={items.loading} />
          {items.data && items.data.rows.length === 0 && <Empty text={t("debt.none")} />}
          {items.data && items.data.rows.length > 0 && (
            <DataTable
              label={t("debt.items")}
              rows={items.data.rows}
              rowKey={rowKey}
              selectedKey={sel ? rowKey(sel) : null}
              onActivate={setSel}
              cols={[
                { key: "kind", label: t("debt.kind"), value: (r) => tv("debt.kind", r.kind), render: (r) => <Badge group="debt.kind" value={r.kind} tone={kindTone(r.kind)} /> },
                {
                  key: "evidence",
                  label: t("debt.evidence"),
                  value: label,
                  render: (r) => (
                    <button className="link mono" onClick={(e) => (e.stopPropagation(), setSel(r))}>
                      {label(r)}
                    </button>
                  ),
                },
                { key: "age", label: t("debt.age"), value: (r) => r.age_days, render: (r) => <span className="num">{r.age_days}</span>, className: "num" },
                { key: "blocks", label: t("debt.blocks"), value: (r) => r.blocks, render: (r) => <span className="num">{r.blocks}</span>, className: "num" },
                { key: "parts", label: t("debt.parts"), value: (r) => r.parts.join(", "), render: (r) => <span className="mono">{r.parts.join(", ")}</span> },
                { key: "suppliers", label: t("debt.suppliers"), value: (r) => r.suppliers.join(", "), render: (r) => r.suppliers.map((s) => s || t("debt.unknown")).join(", ") },
              ]}
            />
          )}
          {items.data && items.data.limitations.map((l) => <p key={l} className="muted">{t(l as never)}</p>)}
        </Panel>
        <div>
          {sel ? (
            <article className="panel" aria-labelledby="debt-sel">
              <h2 id="debt-sel">
                <Badge group="debt.kind" value={sel.kind} tone={kindTone(sel.kind)} /> {label(sel)}
              </h2>
              <p className="muted">
                {t("debt.since")}: {fmtDate(sel.since)} · {t("debt.age")}: {sel.age_days}
              </p>
              <h3>{t("debt.blocks")}</h3>
              <ul className="list">
                {sel.decisions.map((d) => (
                  <li key={d.id}>
                    {d.title} <span className="muted mono">{d.part}</span>
                  </li>
                ))}
              </ul>
              {sel.blocks > sel.decisions.length && <p className="muted">{t("debt.more", { n: sel.blocks - sel.decisions.length })}</p>}
            </article>
          ) : (
            <Empty text={t("debt.select")} />
          )}
        </div>
      </div>
    </div>
  );
}
