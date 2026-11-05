import { useState } from "react";
import { ApiError, type Api } from "./api";
import { KnotHero } from "./brand";
import { useI18n } from "./i18n";
import { Prefs } from "./Prefs";

export function LoginView({ api, onLogin }: { api: Api; onLogin: () => void }) {
  const { t } = useI18n();
  const [f, setF] = useState({ tenant: "", email: "", password: "", totp: "" });
  const [needTotp, setNeedTotp] = useState(false);
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    setBusy(true);
    try {
      await api.login(f.tenant, f.email, f.password, f.totp || undefined);
      setF({ ...f, password: "", totp: "" });
      onLogin();
    } catch (x) {
      if (x instanceof ApiError && x.code === "mfa_required") setNeedTotp(true);
      else setErr(x instanceof ApiError && x.status === 429 ? t("login.rateLimited") : t("login.failed"));
    } finally {
      setBusy(false);
    }
  }
  const field = (k: keyof typeof f, label: string, type = "text", auto = "off") => (
    <label>
      {label}
      <input type={type} required={k !== "totp"} value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} autoComplete={auto} />
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
          {needTotp && field("totp", t("login.totp"), "text", "one-time-code")}
          <button className="primary" disabled={busy}>
            {t("login.submit")}
          </button>
          {err && <p role="alert" className="notice crit">{err}</p>}
          <p className="hint">{t("login.securityNote")}</p>
        </form>
      </main>
    </div>
  );
}
