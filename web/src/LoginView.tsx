import { useRef, useState } from "react";
import { ApiError, type Api } from "./api";
import { KnotHero } from "./brand";
import { useI18n, type Key } from "./i18n";
import { Prefs } from "./Prefs";

/** password → (authenticator code | security key | recovery code) → session. Secrets are cleared after each try. */
type Step = "password" | "totp" | "key" | "recover" | "recovered";

export function LoginView({ api, onLogin }: { api: Api; onLogin: () => void }) {
  const { t } = useI18n();
  const [f, setF] = useState({ tenant: "", email: "", password: "", totp: "", recovery: "" });
  const [step, setStep] = useState<Step>("password");
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  const status = useRef<HTMLParagraphElement>(null);

  function fail(x: unknown) {
    let k: Key = "login.failed";
    if (x instanceof ApiError && x.status === 429) k = "login.rateLimited";
    else if (x instanceof Error && x.message === "webauthn_cancelled") k = "login.keyCancelled";
    else if (x instanceof Error && x.message === "webauthn_unavailable") k = "login.keyUnavailable";
    else if (x instanceof ApiError && x.code === "webauthn_not_enrolled") k = "login.noKeyEnrolled";
    setErr(t(k));
  }

  async function run(fn: () => Promise<void>) {
    setErr("");
    setBusy(true);
    try {
      await fn();
    } catch (x) {
      fail(x);
    } finally {
      setBusy(false);
    }
  }

  function go(next: Step) {
    setErr("");
    setStep(next);
    requestAnimationFrame(() => status.current?.focus());
  }

  function submit(e: React.FormEvent) {
    e.preventDefault();
    if (step === "key") return signInWithKey();
    if (step === "recover") return recover();
    run(async () => {
      try {
        await api.login(f.tenant, f.email, f.password, step === "totp" ? f.totp : undefined);
      } catch (x) {
        if (x instanceof ApiError && x.code === "mfa_required") return go("totp");
        if (x instanceof ApiError && x.code === "webauthn_required") return go("key");
        setF((v) => ({ ...v, totp: "" }));
        throw x;
      }
      setF((v) => ({ ...v, password: "", totp: "" }));
      onLogin();
    });
  }

  function signInWithKey() {
    run(async () => {
      await api.loginWithKey(f.tenant, f.email, f.password);
      setF((v) => ({ ...v, password: "" }));
      onLogin();
    });
  }

  function recover() {
    run(async () => {
      try {
        await api.recover(f.tenant, f.email, f.password, f.recovery.trim());
      } finally {
        setF((v) => ({ ...v, recovery: "" }));
      }
      setF((v) => ({ ...v, password: "" }));
      go("recovered");
    });
  }

  const field = (k: keyof typeof f, label: string, type = "text", auto = "off", extra: React.InputHTMLAttributes<HTMLInputElement> = {}) => (
    <label>
      {label}
      <input type={type} required value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} autoComplete={auto} {...extra} />
    </label>
  );
  const firstFactorLocked = step !== "password";
  const back = (
    <button type="button" className="link" onClick={() => (setF({ ...f, password: "", totp: "", recovery: "" }), go("password"))}>
      {t("login.back")}
    </button>
  );

  return (
    <div className="login-page">
      <KnotHero />
      <div className="login-top">
        <Prefs />
      </div>
      <main className="login-grid">
        <section className="hero-copy">
          <h1 className="wordmark">
            {t("brand.word")}
            <span className="wordmark-ja" lang="ja">
              {t("brand.kanji")}
            </span>
          </h1>
          <p className="hero-tag" lang="ja">
            {t("hero.tagJa")}
          </p>
          <p className="hero-tag-en" lang="en">
            {t("hero.tagEn")}
          </p>
        </section>
        {step === "recovered" ? (
          <section className="login glass" aria-labelledby="login-h">
            <h2 id="login-h">{t("login.recoveredTitle")}</h2>
            <p ref={status} tabIndex={-1} role="status" className="notice">
              {t("login.recoveredText")}
            </p>
            <button className="primary" onClick={onLogin}>
              {t("login.continue")}
            </button>
          </section>
        ) : (
          <form onSubmit={submit} className="login glass" aria-labelledby="login-h">
            <h2 id="login-h">{t("login.title")}</h2>
            {field("tenant", t("login.org"), "text", "organization", { readOnly: firstFactorLocked })}
            {field("email", t("login.email"), "email", "username", { readOnly: firstFactorLocked })}
            {field("password", t("login.password"), "password", "current-password")}
            {step !== "password" && (
              <p ref={status} tabIndex={-1} className="hint step-hint">
                {t(step === "totp" ? "login.totpStep" : step === "key" ? "login.keyStep" : "login.recoverStep")}
              </p>
            )}
            {step === "totp" &&
              field("totp", t("login.totp"), "text", "one-time-code", { inputMode: "numeric", pattern: "[0-9]{6}", maxLength: 6, autoFocus: true })}
            {step === "recover" && field("recovery", t("login.recoveryCode"), "text", "off", { maxLength: 64, autoFocus: true, spellCheck: false })}
            <button className="primary" disabled={busy}>
              {t(step === "key" ? "login.useKey" : step === "recover" ? "login.useRecovery" : "login.submit")}
            </button>
            {step === "key" && (
              <button type="button" className="link" onClick={() => go("recover")}>
                {t("login.lostKey")}
              </button>
            )}
            {step !== "password" && back}
            {busy && step === "key" && (
              <p className="muted" aria-live="polite">
                {t("login.keyWaiting")}
              </p>
            )}
            {err && (
              <p role="alert" className="notice crit">
                {err}
              </p>
            )}
            <p className="hint">{t("login.securityNote")}</p>
          </form>
        )}
      </main>
    </div>
  );
}
