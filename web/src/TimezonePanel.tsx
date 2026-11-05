import { useEffect, useState } from "react";
import { ApiError } from "./api";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Loading, Notice, Panel, useLoad } from "./ui";

// GET/PUT /settings/timezone (admin + MFA, audited). The tenant's IANA timezone decides which calendar day
// "today" is for expiry checks and how report date ranges are read.
export interface TimezoneSetting {
  version: number;
  timezone: string;
  default: string;
  today: string;
}

const PATH = "/settings/timezone";
const FALLBACK = ["Asia/Tokyo", "UTC", "Asia/Seoul", "Asia/Shanghai", "Asia/Singapore", "Asia/Kolkata", "Europe/Berlin", "Europe/London", "America/New_York", "America/Los_Angeles"];

function zones(current: string): string[] {
  let all: string[] = FALLBACK;
  try {
    const f = (Intl as unknown as { supportedValuesOf?: (k: string) => string[] }).supportedValuesOf;
    if (f) all = f("timeZone");
  } catch {
    /* older browsers: fallback list */
  }
  return all.includes(current) ? all : [current, ...all];
}

export function TimezonePanel() {
  const { api, me } = useSession();
  const { t } = useI18n();
  const st = useLoad(() => api.request<TimezoneSetting>("GET", PATH), [api]);
  const [tz, setTz] = useState("");
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const d = st.data;
  useEffect(() => {
    if (d) setTz(d.timezone);
  }, [d]);

  async function save(e: React.FormEvent) {
    e.preventDefault();
    if (!d) return;
    setBusy(true);
    setMsg("");
    setErr(null);
    try {
      await api.request<{ version: number }>("PUT", PATH, { expected_version: d.version, timezone: tz });
      await st.reload();
      setMsg(t("tz.saved", { tz }));
    } catch (x) {
      setErr(x);
      if (x instanceof ApiError && x.code === "settings_version_conflict") await st.reload();
    } finally {
      setBusy(false);
    }
  }

  return (
    <Panel title={t("tz.title")} id="i-tz">
      <p className="muted">{t("tz.lead")}</p>
      <Loading on={st.loading && !d} />
      <Notice msg={msg} error={err ?? st.error} />
      {!me.session_mfa_verified && <p className="callout crit">{t("ai.mfaNeeded")}</p>}
      {d && (
        <form onSubmit={save} className="form-row">
          <label>
            {t("tz.label")}
            <select value={tz} onChange={(e) => setTz(e.target.value)} aria-describedby="tz-hint">
              {zones(d.timezone).map((z) => (
                <option key={z} value={z}>
                  {z}
                </option>
              ))}
            </select>
          </label>
          <span className="hint" id="tz-hint">
            {t("tz.hint", { today: d.today, def: d.default })}
          </span>
          <button className="primary" disabled={busy || !me.session_mfa_verified || tz === d.timezone}>
            {t("tz.save")}
          </button>
        </form>
      )}
    </Panel>
  );
}
