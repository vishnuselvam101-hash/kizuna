import { useState } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Artifact } from "./types";
import { Badge, Empty, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";

export const DOC_KINDS = ["spec_document", "drawing", "purchase_order", "inspection_report", "spreadsheet", "other"] as const;

export function DocumentsView() {
  const { api, can } = useSession();
  const { t, tv, fmtDate, fmtNum } = useI18n();
  const list = useLoad(() => api.request<Artifact[]>("GET", "/artifacts"), []);
  const [kind, setKind] = useState<string>("spec_document");
  const [file, setFile] = useState<File | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);

  async function act(fn: () => Promise<string>) {
    setMsg("");
    setErr(null);
    setBusy(true);
    try {
      setMsg(await fn());
      list.reload();
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }

  function upload(e: React.FormEvent) {
    e.preventDefault();
    if (!file) return;
    act(async () => {
      const fd = new FormData();
      fd.append("kind", kind);
      fd.append("file", file);
      const r = await api.request<{ artifact: Artifact; duplicate: boolean }>("POST", "/artifacts", fd);
      setFile(null);
      return t(r.duplicate ? "docs.duplicate" : "docs.stored", { status: tv("artifact.status", r.artifact.status) });
    });
  }

  return (
    <div>
      <PageTitle title={t("nav.documents")} lead={t("docs.lead")} />
      {can("artifact_upload") && (
        <Panel title={t("docs.upload")} id="up">
          <form onSubmit={upload} className="form-row">
            <label>
              {t("docs.kind")}
              <select value={kind} onChange={(e) => setKind(e.target.value)}>
                {DOC_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {tv("artifact.kind", k)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("docs.file")}
              <input type="file" onChange={(e) => setFile(e.target.files?.[0] ?? null)} />
            </label>
            <button className="primary" disabled={!file || busy}>
              {t("docs.uploadBtn")}
            </button>
          </form>
          <p className="muted">{t("docs.uploadNote")}</p>
        </Panel>
      )}
      <Notice msg={msg} error={err ?? list.error} />
      <Panel title={t("docs.library")} id="lib">
        <Loading on={list.loading} />
        {list.data && list.data.length === 0 && <Empty text={t("docs.none")} />}
        {list.data && list.data.length > 0 && (
          <DataTable
            label={t("docs.library")}
            rows={list.data}
            rowKey={(a) => a.id}
            cols={[
              {
                key: "name",
                label: t("docs.name"),
                value: (a) => a.display_name,
                render: (a) => (
                  <>
                    {a.display_name}
                    {(a.scan_result?.findings ?? []).length > 0 && (
                      <div className="muted">{t("docs.scanFindings", { list: a.scan_result.findings!.join(", ") })}</div>
                    )}
                  </>
                ),
              },
              { key: "kind", label: t("docs.kind"), value: (a) => tv("artifact.kind", a.kind), render: (a) => tv("artifact.kind", a.kind) },
              { key: "size", label: t("docs.size"), value: (a) => a.size_bytes, render: (a) => fmtNum(a.size_bytes), className: "num" },
              { key: "status", label: t("docs.status"), value: (a) => a.status, render: (a) => <Badge group="artifact.status" value={a.status} /> },
              { key: "sha", label: t("docs.sha"), render: (a) => <code title={a.sha256}>{a.sha256.slice(0, 12)}…</code> },
              { key: "at", label: t("docs.captured"), value: (a) => a.created_at, render: (a) => fmtDate(a.created_at), className: "num" },
              {
                key: "act",
                label: t("common.actions"),
                srOnlyLabel: true,
                className: "col-actions",
                render: (a) => (
                  <>
                    <button
                      disabled={busy}
                      aria-label={t("docs.verifyFor", { name: a.display_name })}
                      onClick={() =>
                        act(async () => {
                          const r = await api.request<{ intact: boolean }>("POST", `/artifacts/${a.id}/verify`, {});
                          return t(r.intact ? "docs.intact" : "docs.tampered");
                        })
                      }
                    >
                      {t("docs.verify")}
                    </button>
                    {a.status === "accepted" && can("claim_write") && (
                      <button
                        disabled={busy}
                        aria-label={t("docs.extractFor", { name: a.display_name })}
                        onClick={() =>
                          act(async () => {
                            const r = await api.request<{ claims_created: number; unmapped_labels: string[] }>("POST", `/artifacts/${a.id}/extract`, {});
                            return t("docs.extracted", { n: fmtNum(r.claims_created), labels: r.unmapped_labels.join(", ") || t("common.none") });
                          })
                        }
                      >
                        {t("docs.extract")}
                      </button>
                    )}
                  </>
                ),
              },
            ]}
          />
        )}
      </Panel>
    </div>
  );
}
