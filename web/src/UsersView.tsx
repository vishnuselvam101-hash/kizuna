import { useState } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { ROLES, useSession } from "./session";
import type { User } from "./types";
import { Badge, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";

type Pending = { user: User; action: "disable" | "revoke" | "reset_mfa" } | null;

export function UsersView() {
  const { api, me, can } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const list = useLoad(() => api.request<User[]>("GET", "/users"), []);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const [pending, setPending] = useState<Pending>(null);
  const [f, setF] = useState({ email: "", display_name: "", role: "viewer", password: "" });
  const manage = can("user_manage");

  async function run(fn: () => Promise<string>) {
    setErr(null);
    setMsg("");
    try {
      setMsg(await fn());
      list.reload();
    } catch (x) {
      setErr(x);
    }
  }
  const patch = (u: User, body: Record<string, string>, done: string) =>
    run(async () => {
      await api.request("PATCH", `/users/${u.id}`, body);
      return done;
    });

  function confirm() {
    if (!pending) return;
    const { user: u, action } = pending;
    setPending(null);
    if (action === "disable") patch(u, { status: "disabled" }, t("users.disabled", { email: u.email }));
    else if (action === "revoke")
      run(async () => {
        const r = await api.request<{ revoked: number }>("POST", `/users/${u.id}/revoke-sessions`, {});
        return t("users.revoked", { n: r.revoked, email: u.email });
      });
    else
      run(async () => {
        await api.request("POST", `/users/${u.id}/reset-mfa`, {});
        return t("users.mfaReset", { email: u.email });
      });
  }

  function create(e: React.FormEvent) {
    e.preventDefault();
    run(async () => {
      await api.request("POST", "/users", f);
      setF({ email: "", display_name: "", role: "viewer", password: "" });
      return t("users.created");
    });
  }

  return (
    <div>
      <PageTitle title={t("nav.users")} lead={t("users.lead")} />
      {manage && !me.session_mfa_verified && <p className="callout crit">{t("users.mfaNeeded")}</p>}
      <Notice msg={msg} error={err ?? list.error} />
      {pending && (
        <div className="callout crit confirm" role="alertdialog" aria-labelledby="confirm-h">
          <span id="confirm-h">{t(`users.confirm.${pending.action}` as never, { email: pending.user.email })}</span>
          <button className="danger" onClick={confirm}>
            {t("common.confirm")}
          </button>
          <button onClick={() => setPending(null)}>{t("common.cancel")}</button>
        </div>
      )}
      <Panel title={t("users.list")} id="users">
        <Loading on={list.loading} />
        {list.data && (
          <DataTable
            label={t("users.list")}
            rows={list.data}
            rowKey={(u) => u.id}
            cols={[
              { key: "name", label: t("users.name"), value: (u) => u.display_name, render: (u) => u.display_name },
              { key: "email", label: t("users.email"), value: (u) => u.email, render: (u) => <span className="mono">{u.email}</span> },
              {
                key: "role",
                label: t("users.role"),
                value: (u) => u.role,
                render: (u) =>
                  manage && u.id !== me.user_id ? (
                    <select aria-label={t("users.roleFor", { email: u.email })} value={u.role} onChange={(e) => patch(u, { role: e.target.value }, t("users.roleChanged", { email: u.email }))}>
                      {ROLES.map((r) => (
                        <option key={r} value={r}>
                          {tv("role", r)}
                        </option>
                      ))}
                    </select>
                  ) : (
                    tv("role", u.role)
                  ),
              },
              { key: "status", label: t("docs.status"), value: (u) => u.status, render: (u) => <Badge group="user.status" value={u.status} /> },
              {
                key: "mfa",
                label: t("users.mfa"),
                value: (u) => (u.totp_enabled ? 1 : 0),
                render: (u) => <Badge group="bool" value={u.totp_enabled ? "enabled" : "not_enabled"} tone={u.totp_enabled ? "ok" : "warn"} />,
              },
              { key: "locked", label: t("users.locked"), render: (u) => (u.locked_until ? fmtDate(u.locked_until) : "—"), className: "num" },
              {
                key: "act",
                label: t("common.actions"),
                srOnlyLabel: true,
                className: "col-actions",
                render: (u) =>
                  manage && u.id !== me.user_id ? (
                    <>
                      {u.status === "active" ? (
                        <button className="danger" aria-label={t("users.disableFor", { email: u.email })} onClick={() => setPending({ user: u, action: "disable" })}>
                          {t("users.disable")}
                        </button>
                      ) : (
                        <button aria-label={t("users.enableFor", { email: u.email })} onClick={() => patch(u, { status: "active" }, t("users.enabled", { email: u.email }))}>
                          {t("users.enable")}
                        </button>
                      )}
                      <button aria-label={t("users.revokeFor", { email: u.email })} onClick={() => setPending({ user: u, action: "revoke" })}>
                        {t("users.revoke")}
                      </button>
                      {u.totp_enabled && (
                        <button aria-label={t("users.resetMfaFor", { email: u.email })} onClick={() => setPending({ user: u, action: "reset_mfa" })}>
                          {t("users.resetMfa")}
                        </button>
                      )}
                    </>
                  ) : null,
              },
            ]}
          />
        )}
      </Panel>
      {manage && (
        <Panel title={t("users.create")} id="u-new">
          <form onSubmit={create} className="form-grid">
            <label>
              {t("users.name")}
              <input required value={f.display_name} onChange={(e) => setF({ ...f, display_name: e.target.value })} />
            </label>
            <label>
              {t("users.email")}
              <input required type="email" autoComplete="off" value={f.email} onChange={(e) => setF({ ...f, email: e.target.value })} />
            </label>
            <label>
              {t("users.role")}
              <select value={f.role} onChange={(e) => setF({ ...f, role: e.target.value })}>
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {tv("role", r)}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("users.initialPassword")}
              <input
                required
                type="password"
                autoComplete="new-password"
                minLength={12}
                maxLength={128}
                value={f.password}
                onChange={(e) => setF({ ...f, password: e.target.value })}
                aria-describedby="pw-hint"
              />
              <span id="pw-hint" className="hint">
                {t("users.passwordHint")}
              </span>
            </label>
            <div>
              <button className="primary">{t("users.createBtn")}</button>
            </div>
          </form>
        </Panel>
      )}
    </div>
  );
}
