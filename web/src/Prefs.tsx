import { useEffect, useState } from "react";
import { useI18n } from "./i18n";

export type Theme = "dark" | "light";
const root = () => document.documentElement;

export function setTheme(th: Theme) {
  root().dataset.theme = th;
  root().style.colorScheme = th;
}
export const currentTheme = (): Theme => (root().dataset.theme === "light" ? "light" : "dark");

/** Language and theme toggles. Preferences are held in memory only (no browser storage). */
export function Prefs() {
  const { t, lang, setLang } = useI18n();
  const [theme, setT] = useState<Theme>(currentTheme);
  useEffect(() => {
    const on = () => setT(currentTheme());
    window.addEventListener("kizuna:theme", on);
    return () => window.removeEventListener("kizuna:theme", on);
  }, []);
  const flip = () => {
    setTheme(theme === "dark" ? "light" : "dark");
    window.dispatchEvent(new Event("kizuna:theme"));
  };
  return (
    <div className="prefs">
      <button className="ghost" onClick={() => setLang(lang === "en" ? "ja" : "en")} aria-label={t("prefs.language")} title={t("prefs.language")}>
        <span lang={lang === "en" ? "ja" : "en"}>{t("prefs.otherLang")}</span>
      </button>
      <button className="ghost" onClick={flip} aria-pressed={theme === "light"}>
        {t("prefs.lightTheme")}
      </button>
    </div>
  );
}
