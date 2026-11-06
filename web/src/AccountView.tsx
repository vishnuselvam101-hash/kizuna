import { useEffect, useState } from "react";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { MfaStatus } from "./types";
import { Badge, download, Empty, Loading, Notice, PageTitle, Panel, useLoad } from "./ui";
import { createCredential, webauthnAvailable } from "./webauthn";

/** QR code for the otpauth URI, drawn as one SVG path (encoder loaded on demand, never with the app shell). */
export function QrCode({ text, label }: { text: string; label: string }) {
  const [path, setPath] = useState<{ d: string; n: number } | null>(null);
  useEffect(() => {
    let live = true;
    import("qrcode-generator")
      .then(({ default: qrcode }) => {
        const q = qrcode(0, "M");
        q.addData(text);
        q.make();
        const n = q.getModuleCount();
        let d = "";
        for (let r = 0; r < n; r++) for (let c = 0; c < n; c++) if (q.isDark(r, c)) d += `M${c + 4} ${r + 4}h1v1h-1z`;
        if (live) setPath({ d, n: n + 8 });
      })
      .catch(() => live && setPath(null));
    return () => {
      live = false;
    };
  }, [text]);
  if (!path) return null;
  return (
    <svg className="qr" role="img" aria-label={label} viewBox={`0 0 ${path.n} ${path.n}`} width={200} height={200} shapeRendering="crispEdges">
      <rect width={path.n} height={path.n} fill="#fff" />
      <path d={path.d} fill="#000" />
    </svg>
  );
}

/** Password re-entry for a sensitive change (the server re-checks it and counts failures toward lockout). */
function StepUp({ id, value, onChange }: { id: string; value: string; onChange: (v: string) => void }) {
  const { t } = useI18n();
  return (
    <label>
      {t("account.currentPassword")}
      <input id={id} type="password" required autoComplete="current-password" value={value} onChange={(e) => onChange(e.target.value)} aria-describedby={`${id}-hint`} />
      <span id={`${id}-hint`} className="hint">
        {t("account.stepUpHint")}
      </span>
    </label>
  );
}

/** One-time display of recovery codes; they are never shown again. */
export function RecoveryCodes({ codes, onDone }: { codes: string[]; onDone: () => void }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  const text = codes.join("\n");
  return (
    <section className="callout warn codes-box" aria-labelledby="codes-h">
      <h3 id="codes-h">{t("account.codesTitle")}</h3>
      <p>{t("account.codesOnce")}</p>
      <ol className="codes mono">
        {codes.map((c) => (
          <li key={c}>{c}</li>
        ))}
      </ol>
      <div className="actions">
        <button
          type="button"
          onClick={() =>
            navigator.clipboard?.writeText(text).then(
              () => setCopied(true),
              () => setCopied(false),
            )
          }
        >
          {t("account.codesCopy")}
        </button>
        <button type="button" onClick={() => download("kizuna-recovery-codes.txt", text + "\n", "text/plain")}>
          {t("account.codesDownload")}
        </button>
        <button type="button" className="primary" onClick={onDone}>
          {t("account.codesSaved")}
        </button>
      </div>
      {copied && <p role="status" className="muted">{t("account.copied")}</p>}
    </section>
  );
}

function TotpSection({ enabled, reload }: { enabled: boolean; reload: () => void }) {
  const { api } = useSession();
  const { t } = useI18n();
  const [pw, setPw] = useState("");
  const [setup, setSetup] = useState<{ secret_base32: string; otpauth_uri: string } | null>(null);
  const [code, setCode] = useState("");
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  async function run(fn: () => Promise<void>) {
    setErr(null);
    setMsg("");
    setBusy(true);
    try {
      await fn();
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }
  if (enabled)
    return (
      <Panel title={t("account.totp")} id="acc-totp">
        <Notice msg={msg} />
        <p>
          <Badge group="mfa" value="enabled" tone="ok" /> {t("account.totpOn")}
        </p>
        <p className="muted">{t("account.totpResetNote")}</p>
      </Panel>
    );
  return (
    <Panel title={t("account.totp")} id="acc-totp">
      <Notice msg={msg} error={err} />
      {!setup ? (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            run(async () => {
              const s = await api.request<{ secret_base32: string; otpauth_uri: string }>("POST", "/auth/mfa/totp/setup", { password: pw });
              setPw("");
              setSetup(s);
            });
          }}
        >
          <p className="muted">{t("account.totpLead")}</p>
          <StepUp id="totp-pw" value={pw} onChange={setPw} />
          <button className="primary" disabled={busy}>
            {t("account.totpStart")}
          </button>
        </form>
      ) : (
        <form
          onSubmit={(e) => {
            e.preventDefault();
            run(async () => {
              await api.request("POST", "/auth/mfa/totp/confirm", { code: code.trim() });
              setSetup(null);
              setCode("");
              setMsg(t("account.totpDone"));
              reload();
            });
          }}
        >
          <ol className="steps">
            <li>{t("account.totpScan")}</li>
          </ol>
          <QrCode text={setup.otpauth_uri} label={t("account.qrLabel")} />
          <p>
            {t("account.totpManual")} <code className="secret">{setup.secret_base32.replace(/(.{4})/g, "$1 ").trim()}</code>
          </p>
          <label>
            {t("account.totpCode")}
            <input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} required value={code} onChange={(e) => setCode(e.target.value)} />
          </label>
          <div className="actions">
            <button className="primary" disabled={busy}>
              {t("account.totpVerify")}
            </button>
            <button type="button" onClick={() => (setSetup(null), setCode(""))}>
              {t("common.cancel")}
            </button>
          </div>
        </form>
      )}
    </Panel>
  );
}

function KeysSection({ st, reload, onRelogin }: { st: MfaStatus; reload: () => void; onRelogin: () => void }) {
  const { api, me } = useSession();
  const { t, fmtDate } = useI18n();
  const [add, setAdd] = useState({ label: "", pw: "" });
  const [rm, setRm] = useState<{ id: string; label: string } | null>(null);
  const [rmPw, setRmPw] = useState("");
  const [regenPw, setRegenPw] = useState("");
  const [regenOpen, setRegenOpen] = useState(false);
  const [codes, setCodes] = useState<{ list: string[]; relogin: boolean } | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const needKeySession = st.webauthn_enabled && !me.session_mfa_verified;
  async function run(fn: () => Promise<void>) {
    setErr(null);
    setMsg("");
    setBusy(true);
    try {
      await fn();
    } catch (x) {
      if (x instanceof Error && x.message === "webauthn_cancelled") setMsg(t("login.keyCancelled"));
      else if (x instanceof Error && x.message === "webauthn_unavailable") setMsg(t("login.keyUnavailable"));
      else setErr(x);
    } finally {
      setBusy(false);
    }
  }
  function register(e: React.FormEvent) {
    e.preventDefault();
    run(async () => {
      const s = await api.request<{ challenge_id: string; public_key: { publicKey: Record<string, unknown> } }>("POST", "/auth/mfa/webauthn/register/start", {
        password: add.pw,
      });
      setAdd((a) => ({ ...a, pw: "" }));
      const credential = await createCredential(s.public_key);
      const r = await api.request<{ recovery_codes: string[] | null; relogin_required: boolean }>("POST", "/auth/mfa/webauthn/register/finish", {
        challenge_id: s.challenge_id,
        credential,
        label: add.label.trim() || null,
      });
      setAdd({ label: "", pw: "" });
      // Enrolment ends every session (they were made without the key): show codes first, then sign in again.
      if (r.recovery_codes) setCodes({ list: r.recovery_codes, relogin: true });
      else onRelogin();
    });
  }
  function remove(e: React.FormEvent) {
    e.preventDefault();
    if (!rm) return;
    const target = rm;
    run(async () => {
      const r = await api.request<{ remaining: number }>("POST", `/auth/mfa/webauthn/credentials/${target.id}/remove`, { password: rmPw });
      setRm(null);
      setRmPw("");
      setMsg(r.remaining === 0 ? t("account.keyRemovedLast") : t("account.keyRemoved", { label: target.label }));
      reload();
    });
  }
  function regenerate(e: React.FormEvent) {
    e.preventDefault();
    run(async () => {
      const r = await api.request<{ recovery_codes: string[] }>("POST", "/auth/mfa/recovery-codes", { password: regenPw });
      setRegenPw("");
      setRegenOpen(false);
      setCodes({ list: r.recovery_codes, relogin: false });
      reload();
    });
  }
  const last = st.credentials.length === 1;
  return (
    <>
      <Panel title={t("account.keys")} id="acc-keys">
        <p className="muted">{t("account.keysLead")}</p>
        {needKeySession && <p className="callout warn">{t("account.needKeySession")}</p>}
        <Notice msg={msg} error={err} />
        {codes && (
          <RecoveryCodes
            codes={codes.list}
            onDone={() => {
              const relogin = codes.relogin;
              setCodes(null);
              if (relogin) onRelogin();
            }}
          />
        )}
        {st.credentials.length === 0 ? (
          <Empty text={t("account.noKeys")} />
        ) : (
          <div className="table-wrap" tabIndex={0}>
            <table>
              <caption className="sr-only">{t("account.keys")}</caption>
              <thead>
                <tr>
                  <th scope="col">{t("account.keyLabel")}</th>
                  <th scope="col">{t("account.keyAdded")}</th>
                  <th scope="col">{t("account.keyUsed")}</th>
                  <th scope="col">
                    <span className="sr-only">{t("common.actions")}</span>
                  </th>
                </tr>
              </thead>
              <tbody>
                {st.credentials.map((k) => (
                  <tr key={k.id}>
                    <td>{k.label}</td>
                    <td className="num">{fmtDate(k.created_at)}</td>
                    <td className="num">{k.last_used_at ? fmtDate(k.last_used_at) : t("account.never")}</td>
                    <td className="col-actions">
                      <button className="danger" disabled={busy || needKeySession} aria-label={t("account.removeKeyFor", { label: k.label })} onClick={() => (setRm({ id: k.id, label: k.label }), setRmPw(""))}>
                        {t("account.removeKey")}
                      </button>
                    </td>
                  </tr>
                ))}
              </tbody>
            </table>
          </div>
        )}
        {rm && (
          <form onSubmit={remove} className="review-box" aria-labelledby="rm-h">
            <h3 id="rm-h">{t("account.removeTitle", { label: rm.label })}</h3>
            <p className={last ? "callout crit" : "muted"}>{t(last ? "account.removeLastWarn" : "account.removeWarn")}</p>
            <StepUp id="rm-pw" value={rmPw} onChange={setRmPw} />
            <div className="actions">
              <button className="danger" disabled={busy}>
                {t("account.removeConfirm")}
              </button>
              <button type="button" onClick={() => setRm(null)}>
                {t("common.cancel")}
              </button>
            </div>
          </form>
        )}
        {!needKeySession && (
          <details className="collapsible">
            <summary>{t("account.addKey")}</summary>
            {!webauthnAvailable() && <p className="callout warn">{t("login.keyUnavailable")}</p>}
            <form onSubmit={register} className="form-grid">
              <label>
                {t("account.keyLabel")}
                <input maxLength={100} value={add.label} placeholder={t("account.keyLabelPh")} onChange={(e) => setAdd({ ...add, label: e.target.value })} />
              </label>
              <StepUp id="add-pw" value={add.pw} onChange={(pw) => setAdd({ ...add, pw })} />
              <p className="hint">{t("account.addKeyNote")}</p>
              <div>
                <button className="primary" disabled={busy}>
                  {t("account.addKeyBtn")}
                </button>
              </div>
            </form>
          </details>
        )}
      </Panel>
      {st.webauthn_enabled && (
        <Panel title={t("account.codes")} id="acc-codes">
          <p>{t("account.codesLeft", { n: st.recovery_codes_remaining })}</p>
          {st.recovery_codes_remaining <= 3 && <p className="callout warn">{t("account.codesLow")}</p>}
          {!regenOpen ? (
            <button disabled={busy || needKeySession} onClick={() => setRegenOpen(true)}>
              {t("account.codesRegen")}
            </button>
          ) : (
            <form onSubmit={regenerate} className="review-box">
              <p className="muted">{t("account.codesRegenWarn")}</p>
              <StepUp id="regen-pw" value={regenPw} onChange={setRegenPw} />
              <div className="actions">
                <button className="primary" disabled={busy}>
                  {t("account.codesRegenConfirm")}
                </button>
                <button type="button" onClick={() => (setRegenOpen(false), setRegenPw(""))}>
                  {t("common.cancel")}
                </button>
              </div>
            </form>
          )}
        </Panel>
      )}
    </>
  );
}

export function AccountView({ onRelogin }: { onRelogin: () => void }) {
  const { api, me } = useSession();
  const { t, tv } = useI18n();
  const st = useLoad(() => api.request<MfaStatus>("GET", "/auth/mfa/status"), []);
  return (
    <div>
      <PageTitle title={t("nav.account")} lead={t("account.lead")} />
      <dl className="kv">
        <dt>{t("login.email")}</dt>
        <dd>{me.email}</dd>
        <dt>{t("users.role")}</dt>
        <dd>{tv("role", me.role)}</dd>
        <dt>{t("account.session")}</dt>
        <dd>
          <Badge group="account.session" value={me.session_mfa_verified ? "verified" : "unverified"} tone={me.session_mfa_verified ? "ok" : "warn"} />
        </dd>
      </dl>
      <Loading on={st.loading} />
      <Notice error={st.error} />
      {st.data && (
        <div className="grid2">
          <TotpSection enabled={st.data.totp_enabled} reload={st.reload} />
          <div>
            <KeysSection st={st.data} reload={st.reload} onRelogin={onRelogin} />
          </div>
        </div>
      )}
    </div>
  );
}
