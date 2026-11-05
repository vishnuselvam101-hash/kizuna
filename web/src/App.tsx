import { useCallback, useEffect, useMemo, useRef, useState } from "react";
import { AccountView } from "./AccountView";
import { Api } from "./api";
import { KnotMark } from "./brand";
import { CommandPalette, type Command } from "./CommandPalette";
import { CompareView } from "./CompareView";
import { DashboardView } from "./DashboardView";
import { AffectedView, DecisionsView } from "./DecisionsView";
import { DocumentsView } from "./DocumentsView";
import { FindingsView } from "./FindingsView";
import { I18nProvider, useI18n, type Lang } from "./i18n";
import { IntegrationsView } from "./IntegrationsView";
import { LoginView } from "./LoginView";
import { SECTIONS, sectionOf, type View } from "./nav";
import { currentTheme, Prefs, setTheme } from "./Prefs";
import { RecordsView } from "./RecordsView";
import { SecurityView } from "./SecurityView";
import { ReportsView } from "./ReportsView";
import { roleCan, SessionContext, type Perm, type Session } from "./session";
import type { Me } from "./types";
import { UsersView } from "./UsersView";

const defaultApi = new Api();

function Shell({ api }: { api: Api }) {
  const { t, tv } = useI18n();
  const [me, setMe] = useState<Me | null>(null);
  const [view, setView] = useState<View>("dashboard");
  const main = useRef<HTMLElement>(null);

  // Any session end (logout, refresh failure) drops every piece of tenant data held in React state.
  const endSession = useCallback(() => {
    setMe(null);
    setView("dashboard");
  }, []);
  useEffect(() => {
    api.onSessionEnd = endSession;
    return () => {
      api.onSessionEnd = null;
    };
  }, [api, endSession]);

  const session: Session | null = useMemo(() => (me ? { api, me, can: (p: Perm) => roleCan(me.role, p) } : null), [api, me]);
  const visible = useMemo(
    () => (session ? SECTIONS.map((s) => ({ ...s, views: s.views.filter((v) => !v.perm || session.can(v.perm)) })).filter((s) => s.views.length) : []),
    [session],
  );

  const go = useCallback((v: View) => {
    setView(v);
    // Move focus for screen readers without letting the browser scroll <main> under the sticky bar.
    requestAnimationFrame(() => {
      main.current?.focus({ preventScroll: true });
      if (window.scrollY > 0) window.scrollTo({ top: 0 });
    });
  }, []);
  const logout = useCallback(async () => {
    try {
      await api.logout();
    } finally {
      endSession();
    }
  }, [api, endSession]);

  const commands: Command[] = useMemo(
    () => [
      ...visible.flatMap((s) => s.views.map((v) => ({ id: `go-${v.id}`, label: t(`nav.${v.id}` as never), group: t(`section.${s.id}` as never), run: () => go(v.id) }))),
      {
        id: "theme",
        label: t("prefs.toggleTheme"),
        group: t("palette.prefs"),
        run: () => (setTheme(currentTheme() === "dark" ? "light" : "dark"), window.dispatchEvent(new Event("kizuna:theme"))),
      },
      { id: "go-account", label: t("nav.account"), group: t("palette.session"), run: () => go("account") },
      { id: "logout", label: t("shell.signOut"), group: t("palette.session"), run: logout },
    ],
    [visible, t, go, logout],
  );

  if (!session) {
    return (
      <LoginView
        api={api}
        onLogin={async () => {
          try {
            setMe(await api.request<Me>("GET", "/auth/me"));
          } catch {
            await logout();
          }
        }}
      />
    );
  }

  const section = sectionOf(view);
  const sub = visible.find((s) => s.id === section?.id)?.views ?? [];
  return (
    <SessionContext.Provider value={session}>
      <div className="app">
        <a href="#main" className="skip">
          {t("shell.skip")}
        </a>
        <header className="topbar">
          <div className="brand">
            <KnotMark size={22} />
            <span className="brand-name">{t("brand.product")}</span>
          </div>
          <nav aria-label={t("shell.primaryNav")} className="primary-nav">
            {visible.map((s) => (
              <button key={s.id} aria-current={s.id === section?.id ? "page" : undefined} onClick={() => go(s.views[0].id)}>
                {t(`section.${s.id}` as never)}
              </button>
            ))}
          </nav>
          <div className="topbar-right">
            <button className="ghost kbd-btn" onClick={() => window.dispatchEvent(new Event("kizuna:palette"))} title={t("palette.shortcut")}>
              {t("palette.open")} <kbd>⌘K</kbd>
            </button>
            <Prefs />
            <span className="who" title={session.me.email}>
              {session.me.display_name} · {tv("role", session.me.role)}
            </span>
            <button className="ghost" aria-current={view === "account" ? "page" : undefined} onClick={() => go("account")}>
              {t("nav.account")}
            </button>
            <button className="ghost" onClick={logout}>
              {t("shell.signOut")}
            </button>
          </div>
        </header>
        {sub.length > 1 && (
          <nav aria-label={t("shell.sectionNav")} className="sub-nav">
            {sub.map((v) => (
              <button key={v.id} aria-current={v.id === view ? "page" : undefined} onClick={() => go(v.id)}>
                {t(`nav.${v.id}` as never)}
              </button>
            ))}
          </nav>
        )}
        <main id="main" ref={main} tabIndex={-1} className="content">
          <div key={view} className="reveal">
            {view === "dashboard" && <DashboardView onNavigate={go} />}
            {view === "documents" && <DocumentsView />}
            {view === "records" && <RecordsView />}
            {view === "revisions" && <CompareView />}
            {view === "findings" && <FindingsView />}
            {view === "decisions" && <DecisionsView />}
            {view === "affected" && <AffectedView />}
            {view === "reports" && <ReportsView />}
            {view === "users" && <UsersView />}
            {view === "integrations" && <IntegrationsView />}
            {view === "security" && <SecurityView />}
            {view === "account" && <AccountView onSignOut={logout} />}
          </div>
        </main>
        <CommandPalette commands={commands} />
      </div>
    </SessionContext.Provider>
  );
}

export function App({ api = defaultApi, lang }: { api?: Api; lang?: Lang }) {
  return (
    <I18nProvider initial={lang}>
      <Shell api={api} />
    </I18nProvider>
  );
}
