import { useEffect, useState } from "react";
import { ApiError } from "./api";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Loading, Notice, Panel, useLoad } from "./ui";

// GET/PUT /security/detection-settings (admin + MFA, audited). The server re-validates; the
// client checks the same bounds so an admin sees out-of-range values before saving.
export const THRESHOLDS = [
  "brute_force",
  "brute_force_lock",
  "spray_failures",
  "spray_users",
  "authz_denials",
  "privileged_ops",
  "upload_abuse",
  "extraction_failures",
  "exports",
] as const;
export type ThresholdName = (typeof THRESHOLDS)[number];
export type Thresholds = Record<ThresholdName, number>;

export interface DetectionConfig {
  mode: "static" | "adaptive";
  thresholds: Thresholds;
  adaptive_max_factor: number;
  baseline_days: number;
}

export interface DetectionSettings {
  version: number;
  settings: Partial<DetectionConfig>;
  effective: DetectionConfig;
  defaults: DetectionConfig;
  bounds: Record<string, [number, number]>;
  active_thresholds: Partial<Thresholds>;
}

/** Returns the names of invalid fields ("brute_force_lock" also when below brute_force). */
export function validateDetection(c: DetectionConfig, bounds: Record<string, [number, number]>): string[] {
  const errs: string[] = [];
  const check = (name: string, v: number, integer: boolean) => {
    const b = bounds[name];
    if (!Number.isFinite(v) || (integer && !Number.isInteger(v)) || (b && (v < b[0] || v > b[1]))) errs.push(name);
  };
  THRESHOLDS.forEach((k) => check(k, c.thresholds[k], true));
  if (!errs.includes("brute_force_lock") && c.thresholds.brute_force_lock < c.thresholds.brute_force) errs.push("brute_force_lock");
  if (c.mode === "adaptive") {
    check("adaptive_max_factor", c.adaptive_max_factor, false);
    check("baseline_days", c.baseline_days, true);
  }
  return errs;
}

const PATH = "/security/detection-settings";

export function DetectionSettingsPanel() {
  const { api, me } = useSession();
  const { t, fmtNum } = useI18n();
  const st = useLoad(() => api.request<DetectionSettings>("GET", PATH), [api]);
  const [c, setC] = useState<DetectionConfig | null>(null);
  const [errs, setErrs] = useState<string[]>([]);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const d = st.data;
  useEffect(() => {
    if (d) setC({ ...d.effective, thresholds: { ...d.effective.thresholds } });
  }, [d]);

  async function save() {
    if (!d || !c) return;
    const v = validateDetection(c, d.bounds);
    setErrs(v);
    setMsg("");
    setErr(null);
    if (v.length) return;
    try {
      const r = await api.request<{ version: number }>("PUT", PATH, { expected_version: d.version, settings: c });
      await st.reload();
      setMsg(t("det.saved", { v: r.version }));
    } catch (x) {
      setErr(x);
      if (x instanceof ApiError && x.code === "settings_version_conflict") await st.reload();
    }
  }

  const num = (v: string) => (v === "" ? NaN : Number(v));
  const bound = (k: string) => d?.bounds[k];
  const field = (k: string, label: string, v: number, set: (n: number) => void, def: number, opts: { step?: number; hideLabel?: boolean } = {}) => {
    const b = bound(k);
    const hid = `det-${k}-hint`;
    return (
      <div key={k}>
        <label>
          {opts.hideLabel ? <span className="sr-only">{label}</span> : label}
          <input
            type="number"
            inputMode={opts.step ? "decimal" : "numeric"}
            step={opts.step}
            min={b?.[0]}
            max={b?.[1]}
            value={Number.isNaN(v) ? "" : v}
            aria-invalid={errs.includes(k)}
            aria-describedby={hid}
            onChange={(e) => set(num(e.target.value))}
          />
        </label>
        <span className="hint" id={hid}>
          {b ? t("det.range", { min: b[0], max: b[1], def }) : t("det.default", { def })}
          {k === "brute_force_lock" && ` · ${t("det.lockHint")}`}
        </span>
      </div>
    );
  };

  return (
    <Panel title={t("det.title")} id="s-det">
      <p className="muted">{t("det.lead")}</p>
      <Loading on={st.loading && !d} />
      <Notice msg={msg} error={err ?? st.error} />
      {!me.session_mfa_verified && <p className="callout crit">{t("ai.mfaNeeded")}</p>}
      {d && c && (
        <form
          noValidate
          onSubmit={(e) => {
            e.preventDefault();
            save();
          }}
        >
          <label className="check">
            <input type="checkbox" checked={c.mode === "adaptive"} onChange={(e) => setC({ ...c, mode: e.target.checked ? "adaptive" : "static" })} />
            {t("det.adaptive")}
          </label>
          <p className="hint">{t("det.adaptiveLead")}</p>
          {c.mode === "adaptive" && (
            <div className="form-grid">
              {field("adaptive_max_factor", t("det.factor"), c.adaptive_max_factor, (n) => setC({ ...c, adaptive_max_factor: n }), d.defaults.adaptive_max_factor, { step: 0.1 })}
              {field("baseline_days", t("det.baselineDays"), c.baseline_days, (n) => setC({ ...c, baseline_days: n }), d.defaults.baseline_days)}
            </div>
          )}
          <table className="table">
            <caption className="sr-only">{t("det.title")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("det.rule")}</th>
                <th scope="col">{t("det.threshold")}</th>
                <th scope="col">{t("det.active")}</th>
              </tr>
            </thead>
            <tbody>
              {THRESHOLDS.map((k) => (
                <tr key={k}>
                  <th scope="row">{t(`det.t.${k}`)}</th>
                  <td>
                    {field(
                      k,
                      t("det.thresholdFor", { rule: t(`det.t.${k}`) }),
                      c.thresholds[k],
                      (n) => setC({ ...c, thresholds: { ...c.thresholds, [k]: n } }),
                      d.defaults.thresholds[k],
                      { hideLabel: true },
                    )}
                  </td>
                  <td className="num">{d.active_thresholds[k] === undefined ? "—" : fmtNum(d.active_thresholds[k]!)}</td>
                </tr>
              ))}
            </tbody>
          </table>
          {errs.length > 0 && (
            <p role="alert" className="notice crit">
              {t("det.invalid", { n: errs.length })}
            </p>
          )}
          <div className="actions">
            <button className="primary" disabled={!me.session_mfa_verified}>
              {t("det.save")}
            </button>
            <button type="button" onClick={() => (setC({ ...d.defaults, thresholds: { ...d.defaults.thresholds } }), setErrs([]))}>
              {t("det.reset")}
            </button>
          </div>
        </form>
      )}
    </Panel>
  );
}
