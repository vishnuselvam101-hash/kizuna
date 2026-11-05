import { useState } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Empty, Loading, Notice, Panel, useLoad } from "./ui";

export interface SupplierUser {
  id: string;
  supplier_id: string;
  supplier: string;
  email: string;
  display_name: string;
  status: "invited" | "active" | "revoked";
  invited_at: string;
  invite_expires_at: string;
  activated_at: string | null;
  revoked_at: string | null;
}

/** Tenant admin: invite and revoke supplier portal users (ADR 0023). The invitation link is shown once. */
export function SupplierPortalPanel() {
  const { api, can } = useSession();
  const { t, fmtDate } = useI18n();
  const manage = can("user_manage");
  const list = useLoad(() => api.request<SupplierUser[]>("GET", "/portal-users"), []);
  const suppliers = useLoad(() => api.request<{ id: string; name: string }[]>("GET", "/suppliers"), []);
  const requests = useLoad(
    () => (manage ? api.request<{ id: string; reference: string; supplier: string; status: string; supplier_id: string | null }[]>("GET", "/evidence-requests") : Promise.resolve([])),
    [],
  );
  const NEW = "__new__";
  const [f, setF] = useState({ supplier_id: NEW, supplier: "", email: "", display_name: "" });
  const [att, setAtt] = useState({ request: "", supplier_id: "" });
  const unattached = (requests.data ?? []).filter((r) => !r.supplier_id && r.status !== "cancelled");
  const [link, setLink] = useState<{ email: string; url: string } | null>(null);
  const [pending, setPending] = useState<SupplierUser | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);

  async function run(fn: () => Promise<string>) {
    setErr(null);
    setMsg("");
    try {
      setMsg(await fn());
      list.reload();
      suppliers.reload();
      requests.reload();
    } catch (x) {
      setErr(x);
    }
  }

  function invite(e: React.FormEvent) {
    e.preventDefault();
    run(async () => {
      const body =
        f.supplier_id === NEW
          ? { supplier: f.supplier, email: f.email, display_name: f.display_name }
          : { supplier_id: f.supplier_id, email: f.email, display_name: f.display_name };
      const r = await api.request<{ user: SupplierUser; invite_token: string }>("POST", "/portal-users", body);
      // The token travels in the fragment: browsers never send it to a server or put it in a Referer.
      setLink({ email: r.user.email, url: `${window.location.origin}/portal#invite=${r.invite_token}` });
      setF({ supplier_id: NEW, supplier: "", email: "", display_name: "" });
      return t("supplierUsers.invited", { email: r.user.email });
    });
  }

  function attach(e: React.FormEvent) {
    e.preventDefault();
    const r = unattached.find((x) => x.id === att.request);
    const sup = suppliers.data?.find((x) => x.id === att.supplier_id);
    if (!r || !sup) return;
    run(async () => {
      await api.request("POST", `/evidence-requests/${r.id}/attach-supplier`, { supplier_id: sup.id });
      setAtt({ request: "", supplier_id: "" });
      return t("supplierUsers.attached", { ref: r.reference, supplier: sup.name });
    });
  }

  function revoke() {
    const u = pending;
    setPending(null);
    if (!u) return;
    run(async () => {
      await api.request("POST", `/portal-users/${u.id}/revoke`, {});
      return t("supplierUsers.revoked", { email: u.email });
    });
  }

  const copy = () => {
    if (link && navigator.clipboard) navigator.clipboard.writeText(link.url).then(() => setMsg(t("supplierUsers.copied")), setErr);
  };

  return (
    <Panel title={t("supplierUsers.title")} id="supplier-users">
      <p className="muted">{t("supplierUsers.lead")}</p>
      <Notice msg={msg} error={err ?? list.error} />
      {link && (
        <div className="callout" role="region" aria-label={t("supplierUsers.copy")}>
          <p>{t("supplierUsers.inviteLink", { email: link.email })}</p>
          <code className="mono break">{link.url}</code>
          <div>
            <button onClick={copy}>{t("supplierUsers.copy")}</button>
          </div>
        </div>
      )}
      {pending && (
        <div className="callout crit confirm" role="alertdialog" aria-labelledby="su-confirm-h">
          <span id="su-confirm-h">{t("supplierUsers.confirmRevoke", { email: pending.email })}</span>
          <button className="danger" onClick={revoke}>
            {t("common.confirm")}
          </button>
          <button onClick={() => setPending(null)}>{t("common.cancel")}</button>
        </div>
      )}
      <Loading on={list.loading} />
      {list.data && list.data.length === 0 && <Empty text={t("supplierUsers.none")} />}
      {list.data && list.data.length > 0 && (
        <DataTable
          label={t("supplierUsers.title")}
          rows={list.data}
          rowKey={(u) => u.id}
          cols={[
            { key: "supplier", label: t("supplierUsers.supplier"), value: (u) => u.supplier, render: (u) => u.supplier },
            { key: "name", label: t("supplierUsers.name"), value: (u) => u.display_name, render: (u) => u.display_name },
            { key: "email", label: t("supplierUsers.email"), value: (u) => u.email, render: (u) => <span className="mono">{u.email}</span> },
            {
              key: "status",
              label: t("supplierUsers.status"),
              value: (u) => u.status,
              render: (u) => <Badge group="supplierUsers.status" value={u.status} tone={u.status === "active" ? "ok" : u.status === "revoked" ? "crit" : "warn"} />,
            },
            {
              key: "exp",
              label: t("supplierUsers.expires"),
              render: (u) => (u.status === "invited" ? fmtDate(u.invite_expires_at) : "—"),
              className: "num",
            },
            {
              key: "act",
              label: t("common.actions"),
              srOnlyLabel: true,
              className: "col-actions",
              render: (u) =>
                manage && u.status !== "revoked" ? (
                  <button className="danger" aria-label={t("supplierUsers.revokeFor", { email: u.email })} onClick={() => setPending(u)}>
                    {t("supplierUsers.revoke")}
                  </button>
                ) : null,
            },
          ]}
        />
      )}
      {manage && (
        <form onSubmit={invite} className="form-grid" aria-labelledby="su-invite-h">
          <h3 id="su-invite-h">{t("supplierUsers.invite")}</h3>
          <label>
            {t("supplierUsers.supplier")}
            <select value={f.supplier_id} onChange={(e) => setF({ ...f, supplier_id: e.target.value })}>
              <option value={NEW}>{t("supplierUsers.newSupplier")}</option>
              {suppliers.data?.map((x) => (
                <option key={x.id} value={x.id}>
                  {x.name}
                </option>
              ))}
            </select>
          </label>
          {f.supplier_id === NEW && (
            <label>
              {t("supplierUsers.newSupplierName")}
              <input required maxLength={200} value={f.supplier} onChange={(e) => setF({ ...f, supplier: e.target.value })} />
            </label>
          )}
          <label>
            {t("supplierUsers.name")}
            <input required maxLength={200} value={f.display_name} onChange={(e) => setF({ ...f, display_name: e.target.value })} />
          </label>
          <label>
            {t("supplierUsers.email")}
            <input required type="email" autoComplete="off" value={f.email} onChange={(e) => setF({ ...f, email: e.target.value })} />
          </label>
          <div>
            <button className="primary">{t("supplierUsers.inviteBtn")}</button>
          </div>
        </form>
      )}
      {manage && (
        <form onSubmit={attach} className="form-grid" aria-labelledby="su-attach-h">
          <h3 id="su-attach-h">{t("supplierUsers.attach")}</h3>
          <p className="hint">{t("supplierUsers.attachLead")}</p>
          {requests.data && unattached.length === 0 ? (
            <Empty text={t("supplierUsers.noUnattached")} />
          ) : (
            <>
              <label>
                {t("supplierUsers.request")}
                <select required value={att.request} onChange={(e) => setAtt({ ...att, request: e.target.value })}>
                  <option value="" />
                  {unattached.map((r) => (
                    <option key={r.id} value={r.id}>
                      {r.reference} · {r.supplier}
                    </option>
                  ))}
                </select>
              </label>
              <label>
                {t("supplierUsers.supplier")}
                <select required value={att.supplier_id} onChange={(e) => setAtt({ ...att, supplier_id: e.target.value })}>
                  <option value="" />
                  {suppliers.data?.map((x) => (
                    <option key={x.id} value={x.id}>
                      {x.name}
                    </option>
                  ))}
                </select>
              </label>
              <div>
                <button className="primary">{t("supplierUsers.attachBtn")}</button>
              </div>
            </>
          )}
        </form>
      )}
    </Panel>
  );
}
