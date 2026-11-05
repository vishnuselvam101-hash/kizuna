import { useCallback, useEffect, useRef, useState } from "react";
import { QrCode } from "./AccountView";
import { Api, ApiError } from "./api";
import { KnotMark } from "./brand";
import { I18nProvider, useI18n, type Key, type Lang } from "./i18n";
import { Prefs } from "./Prefs";
import { Badge, Empty, Loading, Notice, useLoad } from "./ui";

// Supplier portal (/portal, ADR 0023). A separate, minimal app for an external supplier user: sign in (password +
// authenticator, always), see the open requests addressed to the supplier, upload documents, follow their status.
// It shares only the API client and visual language with the tenant app; the session token lives in memory only.

export interface PortalDoc {
  name: string;
  items: { part: string; what_ja: string; what_en: string; purchase_order: string | null; po_line: string | null; lot: string | null; heat: string | null }[];
}
export interface PortalRequest {
  id: string;
  reference: string;
  due_on: string | null;
  documents: PortalDoc[];
  body_ja?: string;
  body_en?: string;
}
export interface PortalUpload {
  id: string;
  original_filename: string;
  size_bytes: number;
  status: string;
  reason_code: string | null;
  created_at: string;
}
interface PortalToken {
  access_token: string;
}

const UPLOAD_TONE: Record<string, "ok" | "warn" | "crit" | "neutral"> = {
  received: "neutral",
  scanning: "neutral",
  received_for_review: "ok",
  quarantined: "warn",
  not_accepted: "crit",
};

/** Reads `#invite=<token>` once and removes it from the address bar (it never reaches server logs or history). */
function takeInvite(): string | null {
  if (typeof window === "undefined") return null;
  const m = /^#invite=([A-Za-z0-9._-]{10,200})$/.exec(window.location.hash);
  if (!m) return null;
  window.history.replaceState(null, "", window.location.pathname);
  return m[1];
}

function Frame({ children, onSignOut }: { children: React.ReactNode; onSignOut?: () => void }) {
  const { t } = useI18n();
  return (
    <div className="portal">
      <header className="portal-bar">
        <span className="portal-brand">
          <KnotMark size={24} />
          <span>{t("brand.word")}</span>
          <span className="muted">{t("portal.title")}</span>
        </span>
        <span className="portal-actions">
          <Prefs />
          {onSignOut && (
            <button className="ghost" onClick={onSignOut}>
              {t("portal.signOut")}
            </button>
          )}
        </span>
      </header>
      <main id="main" className="portal-main" tabIndex={-1}>
        {children}
      </main>
    </div>
  );
}

function errKey(x: unknown): Key {
  return x instanceof ApiError && x.status === 429 ? "portal.rateLimited" : "portal.failed";
}

function SignIn({ api, onIn }: { api: Api; onIn: () => void }) {
  const { t } = useI18n();
  const [f, setF] = useState({ tenant: "", email: "", password: "", code: "" });
  const [err, setErr] = useState("");
  const [busy, setBusy] = useState(false);
  async function submit(e: React.FormEvent) {
    e.preventDefault();
    setErr("");
    setBusy(true);
    try {
      const r = await api.request<PortalToken>("POST", "/portal/login", { tenant: f.tenant, email: f.email, password: f.password, totp_code: f.code });
      api.setTokens({ access_token: r.access_token, refresh_token: "" });
      onIn();
    } catch (x) {
      setErr(t(errKey(x)));
    } finally {
      setF((v) => ({ ...v, password: "", code: "" }));
      setBusy(false);
    }
  }
  const field = (k: keyof typeof f, label: string, type: string, auto: string, extra: React.InputHTMLAttributes<HTMLInputElement> = {}) => (
    <label>
      {label}
      <input type={type} required autoComplete={auto} value={f[k]} onChange={(e) => setF({ ...f, [k]: e.target.value })} {...extra} />
    </label>
  );
  return (
    <form className="portal-card glass" onSubmit={submit} aria-labelledby="portal-signin-h">
      <h1 id="portal-signin-h">{t("portal.signIn")}</h1>
      <p className="muted">{t("portal.lead")}</p>
      {field("tenant", t("portal.org"), "text", "organization")}
      {field("email", t("portal.email"), "email", "username")}
      {field("password", t("portal.password"), "password", "current-password")}
      {field("code", t("portal.code"), "text", "one-time-code", { inputMode: "numeric", pattern: "[0-9]{6}", maxLength: 6, "aria-describedby": "portal-code-hint" })}
      <span id="portal-code-hint" className="hint">
        {t("portal.codeHint")}
      </span>
      <button className="primary" disabled={busy}>
        {t("portal.signIn")}
      </button>
      {err && (
        <p role="alert" className="notice crit">
          {err}
        </p>
      )}
      <p className="hint">{t("portal.sessionNote")}</p>
    </form>
  );
}

function AcceptInvite({ api, token, onIn }: { api: Api; token: string; onIn: () => void }) {
  const { t } = useI18n();
  const [password, setPassword] = useState("");
  const [code, setCode] = useState("");
  const [setup, setSetup] = useState<{ email: string; supplier: string; secret_base32: string; otpauth_uri: string } | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [invalid, setInvalid] = useState(false);
  const [busy, setBusy] = useState(false);
  async function run(fn: () => Promise<void>) {
    setErr(null);
    setBusy(true);
    try {
      await fn();
    } catch (x) {
      if (x instanceof ApiError && x.code === "invalid_invite") setInvalid(true);
      else setErr(x);
    } finally {
      setBusy(false);
    }
  }
  const start = (e: React.FormEvent) => {
    e.preventDefault();
    run(async () => {
      setSetup(await api.request("POST", "/portal/invite/start", { token, password }));
      setPassword("");
    });
  };
  const finish = (e: React.FormEvent) => {
    e.preventDefault();
    run(async () => {
      try {
        const r = await api.request<PortalToken>("POST", "/portal/invite/finish", { token, code });
        api.setTokens({ access_token: r.access_token, refresh_token: "" });
        onIn();
      } finally {
        setCode("");
      }
    });
  };
  if (invalid)
    return (
      <section className="portal-card glass" aria-labelledby="inv-h">
        <h1 id="inv-h">{t("portal.invite.title")}</h1>
        <p role="alert" className="notice crit">
          {t("portal.invite.invalid")}
        </p>
      </section>
    );
  return (
    <section className="portal-card glass" aria-labelledby="inv-h">
      <h1 id="inv-h">{t("portal.invite.title")}</h1>
      <p className="muted">{t("portal.invite.lead")}</p>
      <Notice error={err} />
      {!setup ? (
        <form onSubmit={start} className="portal-form">
          <label>
            {t("portal.invite.newPassword")}
            <input
              type="password"
              required
              minLength={12}
              maxLength={128}
              autoComplete="new-password"
              value={password}
              onChange={(e) => setPassword(e.target.value)}
              aria-describedby="inv-pw-hint"
            />
          </label>
          <span id="inv-pw-hint" className="hint">
            {t("portal.invite.passwordHint")}
          </span>
          <button className="primary" disabled={busy}>
            {t("portal.invite.continue")}
          </button>
        </form>
      ) : (
        <form onSubmit={finish} className="portal-form">
          <p>{t("portal.invite.for", { email: setup.email, supplier: setup.supplier })}</p>
          <p>{t("portal.invite.scan")}</p>
          <QrCode text={setup.otpauth_uri} label={t("portal.invite.qr")} />
          <p>
            {t("portal.invite.key")}: <code className="mono break">{setup.secret_base32}</code>
          </p>
          <label>
            {t("portal.code")}
            <input
              required
              inputMode="numeric"
              pattern="[0-9]{6}"
              maxLength={6}
              autoComplete="one-time-code"
              value={code}
              onChange={(e) => setCode(e.target.value)}
            />
          </label>
          <button className="primary" disabled={busy}>
            {t("portal.invite.finish")}
          </button>
        </form>
      )}
    </section>
  );
}

function RequestList({ api, onOpen }: { api: Api; onOpen: (id: string) => void }) {
  const { t, fmtNum } = useI18n();
  const list = useLoad(() => api.request<PortalRequest[]>("GET", "/portal/requests"), [api]);
  return (
    <section aria-labelledby="req-h">
      <h1 id="req-h">{t("portal.requests")}</h1>
      <p className="muted">{t("portal.lead")}</p>
      <Loading on={list.loading} />
      <Notice error={list.error} />
      {list.data && list.data.length === 0 && <Empty text={t("portal.none")} />}
      <ul className="portal-list">
        {list.data?.map((r) => (
          <li key={r.id}>
            <button className="portal-row" onClick={() => onOpen(r.id)} aria-label={t("portal.open", { ref: r.reference })}>
              <span className="mono">{r.reference}</span>
              <span>{r.due_on ? t("portal.due", { date: r.due_on }) : t("portal.noDue")}</span>
              <span className="muted">{t("portal.docCount", { n: fmtNum(r.documents.length) })}</span>
            </button>
          </li>
        ))}
      </ul>
    </section>
  );
}

function RequestDetail({ api, id, onBack }: { api: Api; id: string; onBack: () => void }) {
  const { t, tv, lang, fmtDate, fmtNum } = useI18n();
  const req = useLoad(() => api.request<PortalRequest>("GET", `/portal/requests/${id}`), [api, id]);
  const ups = useLoad(() => api.request<PortalUpload[]>("GET", `/portal/requests/${id}/uploads`), [api, id]);
  const file = useRef<HTMLInputElement>(null);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  async function upload(e: React.FormEvent) {
    e.preventDefault();
    const f = file.current?.files?.[0];
    if (!f) return;
    setBusy(true);
    setErr(null);
    setMsg("");
    try {
      const fd = new FormData();
      fd.append("file", f, f.name);
      const r = await api.request<PortalUpload>("POST", `/portal/requests/${id}/uploads`, fd);
      setMsg(t("portal.sent", { name: r.original_filename }));
      if (file.current) file.current.value = "";
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
      ups.reload();
    }
  }
  if (req.error) {
    return (
      <section>
        <button className="link" onClick={onBack}>
          {t("portal.back")}
        </button>
        <p role="alert" className="notice crit">
          {t("portal.notFound")}
        </p>
      </section>
    );
  }
  const r = req.data;
  return (
    <section aria-labelledby="det-h">
      <button className="link" onClick={onBack}>
        {t("portal.back")}
      </button>
      <Loading on={req.loading} />
      {r && (
        <>
          <h1 id="det-h" className="mono">
            {r.reference}
          </h1>
          <p>{r.due_on ? t("portal.due", { date: r.due_on }) : t("portal.noDue")}</p>
          <div className="panel">
            <h2>{t("portal.requested")}</h2>
            <ul>
              {r.documents.map((d, i) => (
                <li key={i}>
                  <strong>{t("portal.resend", { name: d.name })}</strong>
                  <ul>
                    {d.items.map((it, j) => (
                      <li key={j}>
                        {t("portal.item", { what: lang === "ja" ? it.what_ja : it.what_en, part: it.part })}
                        {[it.purchase_order, it.lot, it.heat].filter(Boolean).length > 0 && (
                          <span className="muted"> · {[it.purchase_order, it.lot, it.heat].filter(Boolean).join(" · ")}</span>
                        )}
                      </li>
                    ))}
                  </ul>
                </li>
              ))}
            </ul>
          </div>
          <details className="panel">
            <summary>{t("portal.letter")}</summary>
            <pre className="portal-letter" lang={lang}>
              {lang === "ja" ? r.body_ja : r.body_en}
            </pre>
          </details>
          <form className="panel portal-form" onSubmit={upload} aria-labelledby="up-h">
            <h2 id="up-h">{t("portal.upload")}</h2>
            <label>
              {t("portal.file")}
              <input ref={file} type="file" required accept=".pdf,.png,.jpg,.jpeg,.tif,.tiff,.csv,.xlsx,application/pdf,image/*" />
            </label>
            <button className="primary" disabled={busy}>
              {busy ? t("portal.sending") : t("portal.send")}
            </button>
            <p className="hint">{t("portal.reviewNote")}</p>
            <Notice msg={msg} error={err} />
          </form>
          <div className="panel">
            <h2>{t("portal.uploads")}</h2>
            <Loading on={ups.loading} />
            {ups.data && ups.data.length === 0 && <Empty text={t("portal.noUploads")} />}
            <ul className="portal-uploads">
              {ups.data?.map((u) => (
                <li key={u.id}>
                  <span className="break">{u.original_filename}</span>
                  <span className="muted">
                    {fmtDate(u.created_at)} · {fmtNum(Math.ceil(u.size_bytes / 1024))} KB
                  </span>
                  <Badge group="upload" value={u.status} tone={UPLOAD_TONE[u.status]} />
                  {u.reason_code && <span className="hint">{tv("upload.reason", u.reason_code)}</span>}
                </li>
              ))}
            </ul>
          </div>
        </>
      )}
    </section>
  );
}

function Portal({ api }: { api: Api }) {
  const [invite, setInvite] = useState<string | null>(takeInvite);
  const [signedIn, setSignedIn] = useState(api.authenticated);
  const [open, setOpen] = useState<string | null>(null);
  const end = useCallback(() => {
    api.setTokens(null);
    setSignedIn(false);
    setOpen(null);
  }, [api]);
  useEffect(() => {
    api.onSessionEnd = end;
    return () => {
      api.onSessionEnd = null;
    };
  }, [api, end]);
  const signOut = async () => {
    try {
      await api.request("POST", "/portal/logout", {});
    } catch {
      /* already ended */
    } finally {
      end();
    }
  };
  const inNow = () => {
    setInvite(null);
    setSignedIn(true);
  };
  if (!signedIn)
    return <Frame>{invite ? <AcceptInvite api={api} token={invite} onIn={inNow} /> : <SignIn api={api} onIn={inNow} />}</Frame>;
  return (
    <Frame onSignOut={signOut}>
      {open ? <RequestDetail api={api} id={open} onBack={() => setOpen(null)} /> : <RequestList api={api} onOpen={setOpen} />}
    </Frame>
  );
}

const defaultApi = new Api();

export function PortalApp({ api = defaultApi, lang }: { api?: Api; lang?: Lang }) {
  return (
    <I18nProvider initial={lang}>
      <Portal api={api} />
    </I18nProvider>
  );
}
