import { useState } from "react";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { Me } from "./types";
import { Badge, Notice, PageTitle, Panel, useLoad } from "./ui";
import { createCredential, webauthnSupported } from "./webauthn";

/** The signed-in user's own second factors: authenticator app (TOTP) and security key (WebAuthn). */
export function AccountView({ onSignOut }: { onSignOut: () => void }) {
  const { api } = useSession();
  const { t } = useI18n();
  const me = useLoad(() => api.request<Me>("GET", "/auth/me"), []);
  return (
    <div>
      <PageTitle title={t("nav.account")} lead={t("acc.lead")} />
      <TotpPanel enabled={!!me.data?.mfa_enabled} onDone={me.reload} />
      <KeyPanel enabled={!!me.data?.webauthn_enabled} onSignOut={onSignOut} />
    </div>
  );
}

function TotpPanel({ enabled, onDone }: { enabled: boolean; onDone: () => void }) {
  const { api } = useSession();
  const { t } = useI18n();
  const [password, setPassword] = useState("");
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
  const start = (e: React.FormEvent) => {
    e.preventDefault();
    run(async () => {
      setSetup(await api.request("POST", "/auth/mfa/totp/setup", { password }));
      setPassword("");
    });
  };
  const confirm = (e: React.FormEvent) => {
    e.preventDefault();
    run(async () => {
      await api.request("POST", "/auth/mfa/totp/confirm", { code: code.trim() });
      setSetup(null);
      setCode("");
      setMsg(t("acc.totpDone"));
      onDone();
    });
  };
  return (
    <Panel title={t("acc.totpTitle")} id="acc-totp">
      <p>
        {t("acc.state")} <Badge group="bool" value={enabled ? "enabled" : "not_enabled"} tone={enabled ? "ok" : "warn"} />
      </p>
      <Notice msg={msg} error={err} />
      {!enabled && !setup && (
        <form onSubmit={start} className="form-row">
          <label>
            {t("acc.password")}
            <input type="password" required autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
          </label>
          <button className="primary" disabled={busy || !password}>
            {t("acc.totpStart")}
          </button>
        </form>
      )}
      {setup && (
        <form onSubmit={confirm}>
          <p className="muted">{t("acc.totpLead")}</p>
          <dl className="kv">
            <dt>{t("acc.totpSecret")}</dt>
            <dd>
              <code data-testid="totp-secret" className="secret">
                {setup.secret_base32.replace(/(.{4})/g, "$1 ").trim()}
              </code>
            </dd>
            <dt>{t("acc.totpUri")}</dt>
            <dd>
              <code data-testid="totp-uri" className="wrap-any">
                {setup.otpauth_uri}
              </code>
            </dd>
          </dl>
          <div className="form-row">
            <label>
              {t("acc.totpCode")}
              <input inputMode="numeric" autoComplete="one-time-code" pattern="[0-9]{6}" maxLength={6} required value={code} onChange={(e) => setCode(e.target.value)} />
            </label>
            <button className="primary" disabled={busy || code.trim().length !== 6}>
              {t("acc.totpConfirm")}
            </button>
          </div>
        </form>
      )}
    </Panel>
  );
}

function KeyPanel({ enabled, onSignOut }: { enabled: boolean; onSignOut: () => void }) {
  const { api } = useSession();
  const { t } = useI18n();
  const [password, setPassword] = useState("");
  const [label, setLabel] = useState("");
  const [codes, setCodes] = useState<string[] | null>(null);
  const [done, setDone] = useState(false);
  const [err, setErr] = useState<unknown>(null);
  const [keyErr, setKeyErr] = useState("");
  const [busy, setBusy] = useState(false);
  const supported = webauthnSupported();
  async function register(e: React.FormEvent) {
    e.preventDefault();
    setErr(null);
    setKeyErr("");
    setBusy(true);
    try {
      const start = await api.request<{ challenge_id: string; public_key: { publicKey: Record<string, unknown> } }>("POST", "/auth/mfa/webauthn/register/start", { password });
      let credential;
      try {
        credential = await createCredential(start.public_key);
      } catch {
        setKeyErr(t("acc.keyCancelled"));
        return;
      }
      const r = await api.request<{ recovery_codes: string[] | null }>("POST", "/auth/mfa/webauthn/register/finish", {
        challenge_id: start.challenge_id,
        credential,
        label: label.trim() || null,
      });
      setPassword("");
      setCodes(r.recovery_codes ?? []);
      setDone(true);
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }
  return (
    <Panel title={t("acc.keyTitle")} id="acc-key">
      <p>
        {t("acc.state")} <Badge group="bool" value={enabled ? "enabled" : "not_enabled"} tone={enabled ? "ok" : "warn"} />
      </p>
      <p className="muted">{t("acc.keyLead")}</p>
      <Notice error={err} />
      {keyErr && (
        <p role="alert" className="notice crit">
          {keyErr}
        </p>
      )}
      {!supported && <p className="callout">{t("login.keyUnsupported")}</p>}
      {supported && !done && (
        <form onSubmit={register} className="form-row">
          <label>
            {t("acc.password")}
            <input type="password" required autoComplete="current-password" value={password} onChange={(e) => setPassword(e.target.value)} />
          </label>
          <label>
            {t("acc.keyLabel")}
            <input maxLength={100} value={label} onChange={(e) => setLabel(e.target.value)} />
          </label>
          <button className="primary" disabled={busy || !password}>
            {t("acc.keyRegister")}
          </button>
        </form>
      )}
      {done && (
        <section className="callout" aria-labelledby="acc-rec-h">
          <h3 id="acc-rec-h">{t("acc.recoveryTitle")}</h3>
          <p role="status">{t("acc.keyDone")}</p>
          {codes && codes.length > 0 && (
            <>
              <p>{t("acc.recoveryLead")}</p>
              <ol className="codes mono">
                {codes.map((c) => (
                  <li key={c}>{c}</li>
                ))}
              </ol>
            </>
          )}
          <button className="primary" onClick={onSignOut}>
            {t("acc.recoverySaved")}
          </button>
        </section>
      )}
    </Panel>
  );
}
