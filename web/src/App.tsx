import { useState } from "react";
import { Api } from "./api";
import { ArtifactsView } from "./ArtifactsView";
import { CompareView, ConflictsView } from "./ConflictsView";
import { DecisionsView } from "./DecisionsView";
import { EvidenceView } from "./EvidenceView";
import { LoginView } from "./LoginView";

const api = new Api();
const TABS = ["Documents", "Evidence", "Findings", "Revisions", "Decisions"] as const;
type Tab = (typeof TABS)[number];

export function App() {
  const [authed, setAuthed] = useState(false);
  const [tab, setTab] = useState<Tab>("Documents");
  if (!authed) return <LoginView api={api} onLogin={() => setAuthed(true)} />;
  return (
    <div>
      <nav>
        {TABS.map((t) => (
          <button key={t} aria-current={t === tab} onClick={() => setTab(t)}>
            {t}
          </button>
        ))}
        <button onClick={() => api.logout().finally(() => setAuthed(false))}>Sign out</button>
      </nav>
      <main>
        {tab === "Documents" && <ArtifactsView api={api} />}
        {tab === "Evidence" && <EvidenceView api={api} />}
        {tab === "Findings" && <ConflictsView api={api} />}
        {tab === "Revisions" && <CompareView api={api} />}
        {tab === "Decisions" && <DecisionsView api={api} />}
      </main>
    </div>
  );
}
