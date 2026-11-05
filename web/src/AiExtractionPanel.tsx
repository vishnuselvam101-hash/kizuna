import { useState } from "react";
import { ApiError } from "./api";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, Loading, Notice, Panel, useLoad } from "./ui";

// GET/PUT /integrations/ai-extraction (admin + MFA, audited). Enabling requires accepting the
// provider data-processing notice of the current version; the notice text is shown exactly as served.
export interface AiExtraction {
  version: number;
  settings: { enabled: boolean; notice_version: string | number | null };
  active: boolean;
  notice: { version: string | number; text: string };
  provider: { configured: boolean; model?: string; host?: string; daily_calls?: number; daily_chars?: number; calls_per_min?: number };
  usage_today: { calls: number; prompt_chars: number };
}

const PATH = "/integrations/ai-extraction";

export function AiExtractionPanel() {
  const { api, me } = useSession();
  const { t, fmtNum } = useI18n();
  const st = useLoad(() => api.request<AiExtraction>("GET", PATH), [api]);
  const [ack, setAck] = useState(false);
  const [busy, setBusy] = useState(false);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const d = st.data;
  const mfaOk = me.session_mfa_verified;
  const needsAccept = !!d && (!d.settings.enabled || String(d.settings.notice_version) !== String(d.notice.version));

  async function put(enabled: boolean) {
    if (!d) return;
    setBusy(true);
    setMsg("");
    setErr(null);
    try {
      const body = enabled ? { expected_version: d.version, enabled, accept_notice: d.notice.version } : { expected_version: d.version, enabled };
      const r = await api.request<{ version: number }>("PUT", PATH, body);
      setAck(false);
      await st.reload();
      setMsg(t(enabled ? "ai.turnedOn" : "ai.turnedOff", { v: r.version }));
    } catch (x) {
      setErr(x);
      // The notice may have changed with the newer version, so the acknowledgement must be given again.
      if (x instanceof ApiError && x.code === "settings_version_conflict") {
        setAck(false);
        await st.reload();
      }
    } finally {
      setBusy(false);
    }
  }

  const limit = (n?: number) => (n === undefined ? t("ai.unlimited") : fmtNum(n));
  return (
    <Panel title={t("ai.title")} id="i-ai">
      <p className="muted">{t("ai.lead")}</p>
      <Loading on={st.loading && !d} />
      <Notice msg={msg} error={err ?? st.error} />
      {!mfaOk && <p className="callout crit">{t("ai.mfaNeeded")}</p>}
      {d && (
        <>
          <p>
            {t("ai.state")}{" "}
            <Badge group="ai.status" value={d.active ? "on" : "off"} tone={d.active ? "ok" : "info"} />
          </p>
          {d.settings.enabled && !d.active && <p className="callout">{t("ai.enabledInactive")}</p>}
          {!d.provider.configured ? (
            <p className="callout">{t("ai.notConfigured")}</p>
          ) : (
            <dl className="kv" aria-label={t("ai.provider")}>
              <div className="kv-row">
                <dt>{t("ai.model")}</dt>
                <dd className="mono">{d.provider.model ?? "—"}</dd>
              </div>
              <div className="kv-row">
                <dt>{t("ai.host")}</dt>
                <dd className="mono">{d.provider.host ?? "—"}</dd>
              </div>
              <div className="kv-row">
                <dt>{t("ai.budget")}</dt>
                <dd>
                  {t("ai.budgetValue", { calls: limit(d.provider.daily_calls), chars: limit(d.provider.daily_chars), rpm: limit(d.provider.calls_per_min) })}
                </dd>
              </div>
              <div className="kv-row">
                <dt>{t("ai.usage")}</dt>
                <dd>{t("ai.usageValue", { calls: fmtNum(d.usage_today.calls), chars: fmtNum(d.usage_today.prompt_chars) })}</dd>
              </div>
            </dl>
          )}
          <h3 id="ai-notice-h">{t("ai.notice", { v: String(d.notice.version) })}</h3>
          <blockquote className="callout" aria-labelledby="ai-notice-h" data-testid="ai-notice" style={{ whiteSpace: "pre-wrap", margin: 0 }}>
            {d.notice.text}
          </blockquote>
          <p className="hint">{t("ai.noticeVerbatim")}</p>
          {d.settings.enabled && (
            <div className="actions">
              <button type="button" disabled={busy || !mfaOk} onClick={() => put(false)}>
                {t("ai.turnOff")}
              </button>
            </div>
          )}
          {needsAccept && (
            <>
              <label className="check">
                <input type="checkbox" checked={ack} disabled={!d.provider.configured || !mfaOk} onChange={(e) => setAck(e.target.checked)} />
                {t("ai.accept", { v: String(d.notice.version) })}
              </label>
              <div className="actions">
                <button type="button" className="primary" disabled={busy || !ack || !d.provider.configured || !mfaOk} onClick={() => put(true)}>
                  {t(d.settings.enabled ? "ai.reaccept" : "ai.turnOn")}
                </button>
              </div>
            </>
          )}
        </>
      )}
    </Panel>
  );
}
