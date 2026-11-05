import { useRef, useState } from "react";
import { ApiError, type Api } from "./api";
import { KnotHero } from "./brand";
import { useI18n } from "./i18n";
import { Prefs } from "./Prefs";
import { getAssertion, webauthnSupported } from "./webauthn";

export function LoginView({ api, onLogin }: { api: Api; onLogin: () => void }) {
  const { t } = useI18n();
  const [f, setF] = useState({ tenant: "", email: "", password: "", totp: "", recovery: "" });
  const [needTotp, setNeedTotp] = useState(false);
  // "key": the account uses a security key; "recovery": the user lost it and enters a one-time recovery code.
  const [step, setStep] = useState<"password" | "key" | "recovery">("password");
  const pending = useRef<AbortController | null>(null);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    setBusy(true);
    try {
      if (step === "recovery") await api.recover(f.tenant, f.email, f.password, f.recovery.trim());
      else if (step === "key") await withKey();
      else await api.login(f.tenant, f.email, f.password, f.totp || undefined);
      setF({ ...f, password: "", totp: "", recovery: "" });
      onLogin();
    } catch (x) {
      if (x instanceof ApiError && x.code === "mfa_required") setNeedTotp(true);
      else if (x instanceof ApiError && x.code === "webauthn_required" && step === "password") {
        // Password accepted, the account is protected by a security key: continue straight to the key.
        setStep("key");
        try {
          await withKey();
          setF({ ...f, password: "", totp: "", recovery: "" });
          onLogin();
        } catch (y) {
          setErr(keyError(y));
        }
      } else setErr(step === "key" ? keyError(x) : x instanceof ApiError && x.status === 429 ? t("login.rateLimited") : t("login.failed"));
    } finally {
      setBusy(false);
    }
  }
  async function withKey() {
    if (!webauthnSupported()) throw new Error("unsupported");
    const ac = new AbortController();
    pending.current = ac;
    try {
      await api.loginWithKey(f.tenant, f.email, f.password, (rcr) => getAssertion(rcr, ac.signal));
    } finally {
      pending.current = null;
    }
  }
  function keyError(x: unknown) {
    if (x instanceof DOMException && x.name === "AbortError") return "";
    if (x instanceof ApiError) return x.status === 429 ? t("login.rateLimited") : t("login.failed");
    return t(x instanceof Error && x.message === "unsupported" ? "login.keyUnsupported" : "login.keyFailed");
  }
  const field = (k: keyof typeof f, label: string, type = "text", auto = "off") => (
    <label>
      {label}
      <input type={type} required={k !== "totp" && k !== "recovery"} value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} autoComplete={auto} />
    </label>
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
        <form onSubmit={submit} className="login glass" aria-labelledby="login-h">
          <h2 id="login-h">{t("login.title")}</h2>
          {field("tenant", t("login.org"), "text", "organization")}
          {field("email", t("login.email"), "email", "username")}
          {field("password", t("login.password"), "password", "current-password")}
          {needTotp && step === "password" && field("totp", t("login.totp"), "text", "one-time-code")}
          {step === "key" && <p className="hint">{t("login.keyPrompt")}</p>}
          {step === "recovery" && field("recovery", t("login.recoveryCode"), "text", "one-time-code")}
          <button className="primary" disabled={busy}>
            {t(step === "key" ? "login.useKey" : "login.submit")}
          </button>
          {step === "key" && (
            <button type="button" className="link" onClick={() => {
                pending.current?.abort();
                setStep("recovery");
                setErr("");
                setBusy(false);
              }}>
              {t("login.useRecovery")}
            </button>
          )}
          {err && <p role="alert" className="notice crit">{err}</p>}
          <p className="hint">{t("login.securityNote")}</p>
        </form>
      </main>
    </div>
  );
}
