import { useCallback, useEffect, useState, type ReactNode } from "react";
import { ApiError } from "./api";
import { useI18n } from "./i18n";

export function useErrorText() {
  const { t, has } = useI18n();
  return (x: unknown): string => {
    if (x instanceof ApiError) {
      const k = `err.${x.code}`;
      return has(k) ? t(k as never) : t("err.generic", { code: `${x.status} ${x.code}` });
    }
    return t("err.network");
  };
}

/** Loads data on mount / when deps change; never caches across sessions. */
export function useLoad<T>(fn: () => Promise<T>, deps: unknown[]) {
  const [data, setData] = useState<T | null>(null);
  const [error, setError] = useState<unknown>(null);
  const [loading, setLoading] = useState(true);
  const reload = useCallback(() => {
    setLoading(true);
    return fn()
      .then((d) => {
        setData(d);
        setError(null);
      })
      .catch(setError)
      .finally(() => setLoading(false));
  }, deps);
  useEffect(() => {
    reload();
  }, [reload]);
  return { data, error, loading, reload };
}

export function Badge({ group, value, tone }: { group: string; value: string; tone?: "ok" | "warn" | "crit" | "info" }) {
  const { tv } = useI18n();
  return <span className={`badge ${tone ?? toneFor(value)}`}>{tv(group, value)}</span>;
}

const CRIT = ["quarantined", "rejected", "open", "revoked", "potentially_affected", "resolved_rejected", "disabled", "failed"];
const WARN = ["needs_evidence", "pending_scan", "unreviewed", "possible_match", "requires_human_review", "changed", "removed", "added"];
const OK = ["accepted", "valid", "active", "resolved_approved", "approved", "succeeded"];
export function toneFor(v: string): "ok" | "warn" | "crit" | "info" {
  if (CRIT.includes(v)) return "crit";
  if (WARN.includes(v)) return "warn";
  if (OK.includes(v)) return "ok";
  return "info";
}

export function Notice({ msg, error }: { msg?: string; error?: unknown }) {
  const errText = useErrorText();
  if (error) return <p role="alert" className="notice crit">{errText(error)}</p>;
  if (msg) return <p role="status" className="notice">{msg}</p>;
  return null;
}

export function Loading({ on }: { on: boolean }) {
  const { t } = useI18n();
  return on ? <p className="muted" aria-live="polite">{t("common.loading")}</p> : null;
}

export function Panel({ title, children, actions, id }: { title: string; children: ReactNode; actions?: ReactNode; id?: string }) {
  const hid = id ? `${id}-h` : undefined;
  return (
    <section className="panel" aria-labelledby={hid}>
      <header className="panel-head">
        <h2 id={hid}>{title}</h2>
        {actions && <div className="actions">{actions}</div>}
      </header>
      {children}
    </section>
  );
}

export function PageTitle({ title, lead }: { title: string; lead?: string }) {
  return (
    <header className="page-title">
      <h1>{title}</h1>
      {lead && <p className="muted">{lead}</p>}
    </header>
  );
}

export function Empty({ text }: { text: string }) {
  return <p className="empty">{text}</p>;
}

/** Triggers a client-side download. Data never leaves the browser except through the API. */
export function download(filename: string, text: string, mime: string) {
  if (typeof URL.createObjectURL !== "function") return;
  const url = URL.createObjectURL(new Blob([text], { type: mime }));
  const a = document.createElement("a");
  a.href = url;
  a.download = filename;
  document.body.appendChild(a);
  a.click();
  a.remove();
  URL.revokeObjectURL(url);
}

export const short = (id: string) => id.slice(0, 8);
