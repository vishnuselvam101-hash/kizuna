import { useEffect, useState } from "react";
import { AiExtractionPanel } from "./AiExtractionPanel";
import { DOC_KINDS } from "./DocumentsView";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Notice, PageTitle, Panel, download } from "./ui";

// Persisted per tenant as versioned settings (GET/PUT /integrations/import-settings, admin + MFA, audited).
// Defaults mirror the extraction service limits (services/extraction worker DEFAULT_LIMITS) and API upload cap.
export interface ImportSettings {
  default_kind: string;
  max_upload_mb: number;
  csv: { delimiter: "," | ";" | "\t"; encoding: "utf-8" | "shift_jis"; header_row: number; max_records: number };
  xlsx: { sheet: string; header_row: number; max_sheets: number; max_records: number };
  pdf: { ocr: boolean; ocr_language: "jpn+eng" | "eng" | "jpn"; max_pages: number };
  mapping: { subject_key: string; predicate: string; value: string; unit: string; revision: string };
}

export const DEFAULT_SETTINGS: ImportSettings = {
  default_kind: "spec_document",
  max_upload_mb: 25,
  csv: { delimiter: ",", encoding: "utf-8", header_row: 1, max_records: 20000 },
  xlsx: { sheet: "", header_row: 1, max_sheets: 50, max_records: 20000 },
  pdf: { ocr: true, ocr_language: "jpn+eng", max_pages: 500 },
  mapping: { subject_key: "part_number", predicate: "attribute", value: "value", unit: "unit", revision: "revision" },
};

export function validateSettings(s: ImportSettings): string[] {
  const errs: string[] = [];
  const range = (name: string, v: number, lo: number, hi: number) => (!Number.isInteger(v) || v < lo || v > hi) && errs.push(name);
  range("max_upload_mb", s.max_upload_mb, 1, 200);
  range("csv.header_row", s.csv.header_row, 0, 100);
  range("csv.max_records", s.csv.max_records, 1, 20000);
  range("xlsx.header_row", s.xlsx.header_row, 0, 100);
  range("xlsx.max_sheets", s.xlsx.max_sheets, 1, 50);
  range("xlsx.max_records", s.xlsx.max_records, 1, 20000);
  range("pdf.max_pages", s.pdf.max_pages, 1, 500);
  Object.entries(s.mapping).forEach(([k, v]) => {
    if (k !== "unit" && k !== "revision" && !v.trim()) errs.push(`mapping.${k}`);
  });
  return errs;
}

export function IntegrationsView() {
  const { t, tv, fmtNum } = useI18n();
  const [s, setS] = useState<ImportSettings>(DEFAULT_SETTINGS);
  const [errs, setErrs] = useState<string[]>([]);
  const { api } = useSession();
  const [version, setVersion] = useState(0);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  useEffect(() => {
    api
      .request<{ version: number; settings: Partial<ImportSettings> }>("GET", "/integrations/import-settings")
      .then((r) => {
        setVersion(r.version);
        if (r.version > 0) setS({ ...DEFAULT_SETTINGS, ...r.settings });
      })
      .catch(setErr);
  }, [api]);
  async function save() {
    const v = validateSettings(s);
    setErrs(v);
    if (v.length) return;
    try {
      const r = await api.request<{ version: number }>("PUT", "/integrations/import-settings", { expected_version: version, settings: s });
      setVersion(r.version);
      setMsg(t("integrations.saved", { v: r.version }));
    } catch (x) {
      setErr(x);
    }
  }
  const num = (v: string) => (v === "" ? NaN : Number(v));
  const invalid = (k: string) => errs.includes(k);
  const UNITS: Record<string, string> = { max_upload_mb: "MB", header_row: "row", max_records: "records", max_sheets: "sheets", max_pages: "pages" };
  const numField = (k: string, label: string, v: number, set: (n: number) => void, min: number, max: number) => {
    const hid = `imp-${k.replace(".", "-")}-hint`;
    const unit = UNITS[k.split(".").pop() ?? ""];
    return (
      <label>
        {label}
        <input
          type="number"
          inputMode="numeric"
          min={min}
          max={max}
          value={Number.isNaN(v) ? "" : v}
          aria-invalid={invalid(k)}
          aria-describedby={hid}
          onChange={(e) => set(num(e.target.value))}
        />
        <span className="hint" id={hid}>
          {t("integrations.range", { min: fmtNum(min), max: fmtNum(max), unit: unit ? t(`unit.${unit}` as never) : "" })}
        </span>
      </label>
    );
  };

  function exportJson(e: React.FormEvent) {
    e.preventDefault();
    const v = validateSettings(s);
    setErrs(v);
    if (v.length === 0) download("kizuna-import-settings.json", JSON.stringify({ version: 1, ...s }, null, 2), "application/json");
  }

  return (
    <div>
      <PageTitle title={t("nav.integrations")} lead={t("integrations.lead")} />
      <Notice msg={msg} error={err} />
      <form onSubmit={exportJson} noValidate>
        <Panel title={t("integrations.general")} id="i-gen">
          <div className="form-grid">
            <label>
              {t("integrations.defaultKind")}
              <select value={s.default_kind} onChange={(e) => setS({ ...s, default_kind: e.target.value })}>
                {DOC_KINDS.map((k) => (
                  <option key={k} value={k}>
                    {tv("artifact.kind", k)}
                  </option>
                ))}
              </select>
            </label>
            {numField("max_upload_mb", t("integrations.maxUpload"), s.max_upload_mb, (n) => setS({ ...s, max_upload_mb: n }), 1, 200)}
          </div>
        </Panel>
        <Panel title={t("integrations.csv")} id="i-csv">
          <div className="form-grid">
            <label>
              {t("integrations.delimiter")}
              <select value={s.csv.delimiter} onChange={(e) => setS({ ...s, csv: { ...s.csv, delimiter: e.target.value as ImportSettings["csv"]["delimiter"] } })}>
                <option value=",">{t("integrations.comma")}</option>
                <option value=";">{t("integrations.semicolon")}</option>
                <option value={"\t"}>{t("integrations.tab")}</option>
              </select>
            </label>
            <label>
              {t("integrations.encoding")}
              <select value={s.csv.encoding} onChange={(e) => setS({ ...s, csv: { ...s.csv, encoding: e.target.value as ImportSettings["csv"]["encoding"] } })}>
                <option value="utf-8">UTF-8</option>
                <option value="shift_jis">Shift_JIS</option>
              </select>
            </label>
            {numField("csv.header_row", t("integrations.headerRow"), s.csv.header_row, (n) => setS({ ...s, csv: { ...s.csv, header_row: n } }), 0, 100)}
            {numField("csv.max_records", t("integrations.maxRecords"), s.csv.max_records, (n) => setS({ ...s, csv: { ...s.csv, max_records: n } }), 1, 20000)}
          </div>
        </Panel>
        <Panel title={t("integrations.xlsx")} id="i-xlsx">
          <div className="form-grid">
            <label>
              {t("integrations.sheet")}
              <input value={s.xlsx.sheet} placeholder={t("integrations.allSheets")} onChange={(e) => setS({ ...s, xlsx: { ...s.xlsx, sheet: e.target.value } })} />
            </label>
            {numField("xlsx.header_row", t("integrations.headerRow"), s.xlsx.header_row, (n) => setS({ ...s, xlsx: { ...s.xlsx, header_row: n } }), 0, 100)}
            {numField("xlsx.max_sheets", t("integrations.maxSheets"), s.xlsx.max_sheets, (n) => setS({ ...s, xlsx: { ...s.xlsx, max_sheets: n } }), 1, 50)}
            {numField("xlsx.max_records", t("integrations.maxRecords"), s.xlsx.max_records, (n) => setS({ ...s, xlsx: { ...s.xlsx, max_records: n } }), 1, 20000)}
          </div>
        </Panel>
        <Panel title={t("integrations.pdf")} id="i-pdf">
          <div className="form-grid">
            <label className="check">
              <input type="checkbox" checked={s.pdf.ocr} onChange={(e) => setS({ ...s, pdf: { ...s.pdf, ocr: e.target.checked } })} />
              {t("integrations.ocr")}
            </label>
            <label>
              {t("integrations.ocrLang")}
              <select
                value={s.pdf.ocr_language}
                disabled={!s.pdf.ocr}
                onChange={(e) => setS({ ...s, pdf: { ...s.pdf, ocr_language: e.target.value as ImportSettings["pdf"]["ocr_language"] } })}
              >
                <option value="jpn+eng">{t("integrations.langJaEn")}</option>
                <option value="jpn">{t("integrations.langJa")}</option>
                <option value="eng">{t("integrations.langEn")}</option>
              </select>
            </label>
            {numField("pdf.max_pages", t("integrations.maxPages"), s.pdf.max_pages, (n) => setS({ ...s, pdf: { ...s.pdf, max_pages: n } }), 1, 500)}
          </div>
        </Panel>
        <Panel title={t("integrations.mapping")} id="i-map">
          <p className="muted">{t("integrations.mappingLead")}</p>
          <div className="form-grid">
            {(Object.keys(s.mapping) as (keyof ImportSettings["mapping"])[]).map((k) => (
              <label key={k}>
                {t(`integrations.map.${k}` as never)}
                <input
                  value={s.mapping[k]}
                  aria-invalid={invalid(`mapping.${k}`)}
                  onChange={(e) => setS({ ...s, mapping: { ...s.mapping, [k]: e.target.value } })}
                />
              </label>
            ))}
          </div>
        </Panel>
        {errs.length > 0 && (
          <p role="alert" className="notice crit">
            {t("integrations.invalid", { n: errs.length })}
          </p>
        )}
        <div className="actions">
          <button type="button" className="primary" onClick={save}>
            {t("integrations.save")}
          </button>
          <button>{t("integrations.export")}</button>
          <button
            type="button"
            className="reset"
            onClick={() => window.confirm(t("integrations.resetConfirm")) && (setS(DEFAULT_SETTINGS), setErrs([]))}
          >
            {t("integrations.reset")}
          </button>
        </div>
      </form>
      <AiExtractionPanel />
    </div>
  );
}
