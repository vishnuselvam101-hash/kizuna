import { useEffect, useRef, useState, type KeyboardEvent } from "react";
import { DataTable } from "./DataTable";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import { Badge, download, Empty, Loading, Notice, PageTitle, Panel, short, useLoad, type Tone } from "./ui";

export interface Citation {
  claim?: string;
  artifact?: string;
}
interface Item {
  kind?: string;
  description?: string;
  evidence?: Citation[];
}
export interface Content {
  members?: { user: string; role: string }[];
  statement?: string;
  is_is_not?: { dimension: string; is: string; is_not: string }[];
  items?: Item[];
  evidence?: Citation[];
  spc?: boolean;
  horizontal?: { target: string; note?: string }[];
  horizontal_na_reason?: string;
}
export interface CapaCase {
  id: string;
  reference: string;
  title: string;
  source_kind: string;
  source_ref: string;
  subject_key: string | null;
  supplier_key: string | null;
  status: string;
  reopened: number;
  closure_requested_by: string | null;
  closed_by: string | null;
  created_by: string;
  created_at: string;
  updated_at: string;
  progress: number;
  overdue_actions: number;
}
export interface Step {
  discipline: number;
  code: string;
  status: string;
  content: Content;
  missing: string[];
  completed_at: string | null;
}
export interface Action {
  id: string;
  description: string;
  owner_id: string;
  owner_name: string | null;
  due_date: string;
  causes: number[];
  status: string;
  evidence: Citation[];
  done_comment: string | null;
  overdue: boolean;
}
export interface EvidenceInfo {
  type: "claim" | "artifact";
  subject?: string;
  predicate?: string;
  value?: string | null;
  unit?: string | null;
  document?: string | null;
  kind?: string;
  usable: boolean;
  origin: "internal" | "supplier" | null;
}
export interface CapaDetail {
  case: CapaCase;
  source: Record<string, unknown>;
  source_links: Citation[];
  steps: Step[];
  actions: Action[];
  supplier_requests: { id: string; reference: string; supplier: string; disciplines: number[]; due_date: string; status: string; created_by: string }[];
  supplier_responses: { request: string; artifact: string; document: string; added_at: string }[];
  events: { at: string; actor: string | null; actor_kind: string; kind: string; discipline: number | null; detail: Record<string, unknown> }[];
  evidence: Record<string, EvidenceInfo>;
  candidates: string[];
  users: { id: string; name: string; role: string }[];
  spc_recheck: string | null;
}
interface Notification {
  id: string;
  case_id: string;
  reference: string;
  description: string;
  kind: string;
  due_date: string;
  read_at: string | null;
}

const STATUSES = ["open", "closure_pending", "closed"] as const;
const SOURCES = ["conflict", "match_finding", "integrity_signal", "drift_finding", "spc"] as const;
const ROLES = ["leader", "member", "champion"];
const DIMS = ["what", "where", "when", "how_many"];
const CAUSE_KINDS = ["occurrence", "escape"];
const MEASURE_KINDS = ["standard", "fmea", "control_plan", "training", "other"];
const statusTone = (s: string): Tone => (s === "closed" ? "ok" : s === "closure_pending" ? "warn" : "info");
const keyOf = (c: Citation) => c.claim ?? c.artifact ?? "";

function useGuard() {
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  /** Runs `fn`, showing its error; resolves to whether it succeeded. */
  const guard = async (fn: () => Promise<void>): Promise<boolean> => {
    setErr(null);
    setMsg("");
    try {
      await fn();
      return true;
    } catch (x) {
      setErr(x);
      return false;
    }
  };
  return { msg, setMsg, err, guard };
}

/** One line describing a cited claim or document. */
function EvidenceLabel({ id, info }: { id: string; info?: EvidenceInfo }) {
  const { t } = useI18n();
  if (!info) return <code>{short(id)}</code>;
  return (
    <span>
      {info.type === "claim" ? (
        <>
          <span className="mono">{info.predicate}</span> <b className="mono num">{[info.value, info.unit].filter(Boolean).join(" ")}</b>
          {info.document && <span className="muted"> — {info.document}</span>}
        </>
      ) : (
        <>{info.document}</>
      )}{" "}
      {info.origin === "supplier" && <Badge group="capa.origin" value={t("capa.supplierOrigin")} tone="warn" />}
      {!info.usable && <Badge group="capa.origin" value={t("capa.notUsable")} tone="crit" />}
    </span>
  );
}

/** Multi-select of the evidence known to the case plus a free ID field. */
function CitePicker({ id, label, value, onChange, d, disabled }: { id: string; label: string; value: Citation[]; onChange: (v: Citation[]) => void; d: CapaDetail; disabled: boolean }) {
  const { t } = useI18n();
  const [other, setOther] = useState("");
  const chosen = new Set(value.map(keyOf));
  const pool = Array.from(new Set([...d.source_links.map(keyOf), ...d.candidates, ...d.supplier_responses.map((r) => r.artifact), ...chosen])).filter(Boolean);
  const toggle = (k: string) => {
    if (chosen.has(k)) return onChange(value.filter((c) => keyOf(c) !== k));
    onChange([...value, d.evidence[k]?.type === "artifact" ? { artifact: k } : { claim: k }]);
  };
  return (
    <fieldset className="claim-picker" disabled={disabled}>
      <legend>{label}</legend>
      <ul className="picker-list">
        {pool.map((k) => (
          <li key={k}>
            <label className="check">
              <input type="checkbox" checked={chosen.has(k)} onChange={() => toggle(k)} />
              <EvidenceLabel id={k} info={d.evidence[k]} />
            </label>
          </li>
        ))}
      </ul>
      <div className="actions">
        <label>
          {t("capa.citeOther")}
          <input id={`${id}-other`} value={other} onChange={(e) => setOther(e.target.value.trim())} className="mono" />
        </label>
        <button
          type="button"
          onClick={() => {
            if (other && !chosen.has(other)) onChange([...value, { claim: other }]);
            setOther("");
          }}
        >
          {t("capa.add")}
        </button>
      </div>
    </fieldset>
  );
}

function ItemsEditor({ d, n, items, setItems, kinds, cite, disabled }: { d: CapaDetail; n: number; items: Item[]; setItems: (v: Item[]) => void; kinds: string[] | null; cite: boolean; disabled: boolean }) {
  const { t, tv } = useI18n();
  const set = (i: number, patch: Partial<Item>) => setItems(items.map((x, j) => (j === i ? { ...x, ...patch } : x)));
  return (
    <section aria-labelledby={`items-${n}`}>
      <h4 id={`items-${n}`}>{t(`capa.items.${n}` as never)}</h4>
      <ol className="list">
        {items.map((it, i) => (
          <li key={i}>
            <div className="toolbar">
              {kinds && (
                <label>
                  {t("capa.kind")}
                  <select value={it.kind ?? ""} disabled={disabled} onChange={(e) => set(i, { kind: e.target.value })}>
                    <option value="" />
                    {kinds.map((k) => (
                      <option key={k} value={k}>
                        {tv("capa.kind", k)}
                      </option>
                    ))}
                  </select>
                </label>
              )}
              <label className="stack grow">
                {t("capa.description")}
                <input value={it.description ?? ""} disabled={disabled} onChange={(e) => set(i, { description: e.target.value })} />
              </label>
              {!disabled && (
                <button type="button" onClick={() => setItems(items.filter((_, j) => j !== i))}>
                  {t("capa.remove")}
                </button>
              )}
            </div>
            {cite && <CitePicker id={`c${n}-${i}`} label={`${t("capa.cite")} — ${n === 4 ? t("capa.cause", { n: i + 1 }) : i + 1}`} value={it.evidence ?? []} onChange={(v) => set(i, { evidence: v })} d={d} disabled={disabled} />}
          </li>
        ))}
      </ol>
      {!disabled && (
        <button type="button" onClick={() => setItems([...items, { kind: kinds ? kinds[0] : undefined, description: "", evidence: [] }])}>
          {t("capa.addItem")}
        </button>
      )}
    </section>
  );
}

function StepEditor({ d, step, draft, setDraft, editable }: { d: CapaDetail; step: Step; draft: Content; setDraft: (c: Content) => void; editable: boolean }) {
  const { t, tv } = useI18n();
  const n = step.discipline;
  const dis = !editable;
  const set = (patch: Partial<Content>) => setDraft({ ...draft, ...patch });
  const statement = [2, 5, 6, 8].includes(n) && (
    <label className="stack">
      {t(`capa.statement.${n}` as never)}
      <textarea value={draft.statement ?? ""} disabled={dis} onChange={(e) => set({ statement: e.target.value })} />
    </label>
  );
  if (n === 1) {
    const members = draft.members ?? [];
    const setM = (i: number, p: Partial<{ user: string; role: string }>) => set({ members: members.map((m, j) => (j === i ? { ...m, ...p } : m)) });
    return (
      <section aria-labelledby="team-h">
        <h4 id="team-h">{t("capa.members")}</h4>
        {members.map((m, i) => (
          <div className="toolbar" key={i}>
            <label>
              {t("capa.member")}
              <select value={m.user} disabled={dis} onChange={(e) => setM(i, { user: e.target.value })}>
                <option value="" />
                {d.users.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("capa.role")}
              <select value={m.role} disabled={dis} onChange={(e) => setM(i, { role: e.target.value })}>
                {ROLES.map((r) => (
                  <option key={r} value={r}>
                    {tv("capa.role", r)}
                  </option>
                ))}
              </select>
            </label>
          </div>
        ))}
        {editable && (
          <button type="button" onClick={() => set({ members: [...members, { user: "", role: members.length ? "member" : "leader" }] })}>
            {t("capa.addMember")}
          </button>
        )}
      </section>
    );
  }
  if (n === 2) {
    const rows = draft.is_is_not ?? [];
    const setR = (i: number, p: Partial<{ dimension: string; is: string; is_not: string }>) => set({ is_is_not: rows.map((r, j) => (j === i ? { ...r, ...p } : r)) });
    return (
      <>
        {statement}
        <h4>{t("capa.isIsNot")}</h4>
        <div className="table-wrap" tabIndex={0} role="region" aria-label={t("capa.isIsNot")}>
          <table>
            <caption className="sr-only">{t("capa.isIsNot")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("capa.dimension")}</th>
                <th scope="col">{t("capa.is")}</th>
                <th scope="col">{t("capa.isNot")}</th>
              </tr>
            </thead>
            <tbody>
              {rows.map((r, i) => (
                <tr key={i}>
                  <td>
                    <select aria-label={t("capa.dimension")} value={r.dimension} disabled={dis} onChange={(e) => setR(i, { dimension: e.target.value })}>
                      {DIMS.map((x) => (
                        <option key={x} value={x}>
                          {tv("capa.dim", x)}
                        </option>
                      ))}
                    </select>
                  </td>
                  <td>
                    <input aria-label={`${t("capa.is")} ${i + 1}`} value={r.is} disabled={dis} onChange={(e) => setR(i, { is: e.target.value })} />
                  </td>
                  <td>
                    <input aria-label={`${t("capa.isNot")} ${i + 1}`} value={r.is_not} disabled={dis} onChange={(e) => setR(i, { is_not: e.target.value })} />
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
        {editable && (
          <button type="button" onClick={() => set({ is_is_not: [...rows, { dimension: rows.length ? "where" : "what", is: "", is_not: "" }] })}>
            {t("capa.addRow")}
          </button>
        )}
        <CitePicker id="c2" label={t("capa.cite")} value={draft.evidence ?? []} onChange={(v) => set({ evidence: v })} d={d} disabled={dis} />
      </>
    );
  }
  if (n === 3 || n === 4) return <ItemsEditor d={d} n={n} items={draft.items ?? []} setItems={(v) => set({ items: v })} kinds={n === 4 ? CAUSE_KINDS : null} cite disabled={dis} />;
  if (n === 5) return <>{statement}</>;
  if (n === 6)
    return (
      <>
        {statement}
        <CitePicker id="c6" label={t("capa.cite")} value={draft.evidence ?? []} onChange={(v) => set({ evidence: v })} d={d} disabled={dis} />
        {d.case.source_kind === "spc" && (
          <p>
            <label className="check">
              <input type="checkbox" checked={!!draft.spc} disabled={dis} onChange={(e) => set({ spc: e.target.checked })} /> {t("capa.spcCheck")}
            </label>{" "}
            {d.spc_recheck && (
              <>
                {t("capa.spcRecheck")}: <Badge group="capa.spc" value={d.spc_recheck} tone={d.spc_recheck === "incontrol" ? "ok" : "warn"} />
              </>
            )}
          </p>
        )}
      </>
    );
  if (n === 7) {
    const hz = draft.horizontal ?? [];
    return (
      <>
        <ItemsEditor d={d} n={7} items={draft.items ?? []} setItems={(v) => set({ items: v })} kinds={MEASURE_KINDS} cite={false} disabled={dis} />
        <h4>{t("capa.horizontal")}</h4>
        {hz.map((h, i) => (
          <div className="toolbar" key={i}>
            <label>
              {t("capa.target")}
              <input value={h.target} disabled={dis} onChange={(e) => set({ horizontal: hz.map((x, j) => (j === i ? { ...x, target: e.target.value } : x)) })} />
            </label>
            <label>
              {t("capa.note")}
              <input value={h.note ?? ""} disabled={dis} onChange={(e) => set({ horizontal: hz.map((x, j) => (j === i ? { ...x, note: e.target.value } : x)) })} />
            </label>
          </div>
        ))}
        {editable && (
          <button type="button" onClick={() => set({ horizontal: [...hz, { target: "", note: "" }] })}>
            {t("capa.addRow")}
          </button>
        )}
        <label className="stack">
          {t("capa.horizontalNa")}
          <input value={draft.horizontal_na_reason ?? ""} disabled={dis} onChange={(e) => set({ horizontal_na_reason: e.target.value })} />
        </label>
      </>
    );
  }
  return <>{statement}</>;
}

/** Strips empty optional fields so the server's strict schema accepts the draft. */
function clean(c: Content): Content {
  const out: Content = {};
  for (const [k, v] of Object.entries(c)) {
    if (v === undefined || v === "" || v === false || (Array.isArray(v) && v.length === 0)) continue;
    (out as Record<string, unknown>)[k] = v;
  }
  return out;
}

function Actions({ d, editable, onAdd, onDone }: { d: CapaDetail; editable: { add: boolean; done: boolean }; onAdd: (b: unknown) => void; onDone: (id: string, ev: Citation[], comment: string) => Promise<boolean> }) {
  const { t, tv } = useI18n();
  const causes = d.steps[3].content.items ?? [];
  const [desc, setDesc] = useState("");
  const [owner, setOwner] = useState("");
  const [due, setDue] = useState("");
  const [sel, setSel] = useState<number[]>([]);
  const [doing, setDoing] = useState<string | null>(null);
  const [ev, setEv] = useState<Citation[]>([]);
  const [comment, setComment] = useState("");
  return (
    <section aria-labelledby="acts-h">
      <h4 id="acts-h">{t("capa.actions")}</h4>
      {d.actions.length > 0 && (
        <div className="table-wrap" tabIndex={0}>
          <table>
            <caption className="sr-only">{t("capa.actions")}</caption>
            <thead>
              <tr>
                <th scope="col">{t("capa.description")}</th>
                <th scope="col">{t("capa.owner")}</th>
                <th scope="col">{t("capa.due")}</th>
                <th scope="col">{t("capa.causes")}</th>
                <th scope="col">{t("findings.status")}</th>
              </tr>
            </thead>
            <tbody>
              {d.actions.map((a) => (
                <tr key={a.id}>
                  <td>{a.description}</td>
                  <td>{a.owner_name ?? short(a.owner_id)}</td>
                  <td className="num">
                    {a.due_date} {a.overdue && <Badge group="capa" value={t("capa.overdue")} tone="crit" />}
                  </td>
                  <td>{a.causes.map((c) => t("capa.cause", { n: c + 1 })).join(", ")}</td>
                  <td>
                    <Badge group="capa.actionStatus" value={a.status} tone={a.status === "done" ? "ok" : "neutral"} />{" "}
                    {editable.done && a.status === "open" && (
                      <button type="button" onClick={() => (setDoing(a.id), setEv([]), setComment(""))}>
                        {t("capa.markDone")}
                      </button>
                    )}
                  </td>
                </tr>
              ))}
            </tbody>
          </table>
        </div>
      )}
      {doing && (
        <div className="claim-review">
          <CitePicker id="act-ev" label={t("capa.cite")} value={ev} onChange={setEv} d={d} disabled={false} />
          <label className="stack">
            {t("capa.doneComment")}
            <input value={comment} onChange={(e) => setComment(e.target.value)} />
          </label>
          <button
            type="button"
            className="primary"
            onClick={async () => {
              // The form stays open (with its citations and comment) when the server refuses the record.
              if (await onDone(doing, ev, comment)) setDoing(null);
            }}
          >
            {t("capa.markDone")}
          </button>
        </div>
      )}
      {editable.add && (
        <form
          className="claim-review"
          aria-label={t("capa.addAction")}
          onSubmit={(e) => {
            e.preventDefault();
            onAdd({ description: desc, owner, due_date: due, causes: sel });
            setDesc("");
          }}
        >
          <label className="stack">
            {t("capa.description")}
            <input value={desc} onChange={(e) => setDesc(e.target.value)} required />
          </label>
          <div className="toolbar">
            <label>
              {t("capa.owner")}
              <select value={owner} onChange={(e) => setOwner(e.target.value)} required>
                <option value="" />
                {d.users.map((u) => (
                  <option key={u.id} value={u.id}>
                    {u.name}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("capa.due")}
              <input type="date" value={due} onChange={(e) => setDue(e.target.value)} required />
            </label>
          </div>
          <fieldset className="faulty">
            <legend>{t("capa.causes")}</legend>
            {causes.map((c, i) => (
              <label key={i} className="check">
                <input type="checkbox" checked={sel.includes(i)} onChange={() => setSel(sel.includes(i) ? sel.filter((x) => x !== i) : [...sel, i])} />
                {t("capa.cause", { n: i + 1 })} ({tv("capa.kind", c.kind ?? "")}): {c.description}
              </label>
            ))}
          </fieldset>
          <button type="submit">{t("capa.addAction")}</button>
        </form>
      )}
    </section>
  );
}

function Supplier({ d, canWrite, canReview, act }: { d: CapaDetail; canWrite: boolean; canReview: boolean; act: (method: string, path: string, body: unknown, msg: string) => void }) {
  const { api } = useSession();
  const { t, tv } = useI18n();
  const [due, setDue] = useState("");
  const [ds, setDs] = useState<number[]>([3, 4, 5]);
  const [req, setReq] = useState("");
  const [art, setArt] = useState("");
  const [reason, setReason] = useState("");
  const approved = d.supplier_requests.filter((r) => r.status === "approved");
  const letter = async (id: string, ref: string, lang: "ja" | "en") => {
    download(`${ref}-${lang}.txt`, await api.blob("GET", `/capa/supplier-requests/${id}/document?lang=${lang}`), "text/plain;charset=utf-8");
  };
  return (
    <Panel title={t("capa.supplier")} id="cs">
      <p className="muted">{t("capa.supplierNote")}</p>
      <ul className="list">
        {d.supplier_requests.map((r) => (
          <li key={r.id}>
            <span className="mono">{r.reference}</span> {r.supplier} · D{r.disciplines.join("/D")} · {t("capa.due")} <span className="num">{r.due_date}</span>{" "}
            <Badge group="capa.reqStatus" value={r.status} tone={r.status === "approved" ? "ok" : r.status === "draft" ? "warn" : "neutral"} />{" "}
            <button type="button" onClick={() => letter(r.id, r.reference, "ja")}>
              {t("capa.downloadJa")}
            </button>{" "}
            <button type="button" onClick={() => letter(r.id, r.reference, "en")}>
              {t("capa.downloadEn")}
            </button>
            {canReview && r.status === "draft" && (
              <div className="actions">
                <button type="button" onClick={() => act("POST", `/capa/supplier-requests/${r.id}/approve`, {}, t("capa.approve"))}>
                  {t("capa.approve")}
                </button>
                <label>
                  {t("capa.cancelReason")}
                  <input value={reason} onChange={(e) => setReason(e.target.value)} />
                </label>
                <button type="button" onClick={() => act("POST", `/capa/supplier-requests/${r.id}/cancel`, { comment: reason }, t("capa.cancelRequest"))}>
                  {t("capa.cancelRequest")}
                </button>
              </div>
            )}
          </li>
        ))}
      </ul>
      {d.supplier_responses.length > 0 && (
        <>
          <h3>{t("capa.responses")}</h3>
          <ul className="list">
            {d.supplier_responses.map((r) => (
              <li key={r.artifact}>
                {r.document} <Badge group="capa.origin" value={t("capa.supplierOrigin")} tone="warn" />
              </li>
            ))}
          </ul>
        </>
      )}
      {canWrite && d.case.status === "open" && (
        <form
          aria-label={t("capa.draftRequest")}
          onSubmit={(e) => {
            e.preventDefault();
            act("POST", `/capa/cases/${d.case.id}/supplier-requests`, { due_date: due, disciplines: ds }, t("capa.requestDrafted"));
          }}
        >
          <div className="toolbar">
            <label>
              {t("capa.due")}
              <input type="date" value={due} onChange={(e) => setDue(e.target.value)} required />
            </label>
            <fieldset className="faulty">
              <legend>{t("capa.disciplines")}</legend>
              {[3, 4, 5, 6, 7].map((n) => (
                <label key={n} className="check">
                  <input type="checkbox" checked={ds.includes(n)} onChange={() => setDs(ds.includes(n) ? ds.filter((x) => x !== n) : [...ds, n].sort())} />
                  {tv("capa.d", String(n))}
                </label>
              ))}
            </fieldset>
            <button type="submit">{t("capa.draftRequest")}</button>
          </div>
        </form>
      )}
      {canWrite && approved.length > 0 && d.case.status !== "closed" && (
        <form
          aria-label={t("capa.attach")}
          onSubmit={(e) => {
            e.preventDefault();
            act("POST", `/capa/cases/${d.case.id}/supplier-responses`, { request: req || approved[0].id, artifact: art.trim() }, t("capa.attached"));
          }}
        >
          <div className="toolbar">
            <label>
              {t("capa.request")}
              <select value={req} onChange={(e) => setReq(e.target.value)}>
                {approved.map((r) => (
                  <option key={r.id} value={r.id}>
                    {r.reference}
                  </option>
                ))}
              </select>
            </label>
            <label>
              {t("capa.artifactId")}
              <input className="mono" value={art} onChange={(e) => setArt(e.target.value)} required />
            </label>
            <button type="submit">{t("capa.attach")}</button>
          </div>
        </form>
      )}
    </Panel>
  );
}

export function CaseView({ d, onChange }: { d: CapaDetail; onChange: (d: CapaDetail) => void }) {
  const { api, can, me } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const firstOpen = d.steps.find((s) => s.status === "open")?.discipline ?? 8;
  const [sel, setSel] = useState(firstOpen);
  const step = d.steps[sel - 1];
  const [draft, setDraft] = useState<Content>(step.content);
  const [reason, setReason] = useState("");
  const [closeComment, setCloseComment] = useState("");
  const { msg, setMsg, err, guard } = useGuard();
  const tabs = useRef<(HTMLButtonElement | null)[]>([]);
  useEffect(() => setDraft(d.steps[sel - 1].content), [sel, d]);
  const c = d.case;
  const canWrite = can("decision_write");
  const editable = canWrite && c.status === "open" && step.status === "open";
  const act = (method: string, path: string, body: unknown, done: string) =>
    guard(async () => {
      onChange(await api.request<CapaDetail>(method, path, body));
      setMsg(done);
    });
  const onKey = (e: KeyboardEvent<HTMLButtonElement>) => {
    const k = { ArrowRight: 1, ArrowLeft: -1, Home: -8, End: 8 }[e.key];
    if (!k) return;
    e.preventDefault();
    const next = Math.min(8, Math.max(1, sel + k));
    setSel(next);
    tabs.current[next - 1]?.focus();
  };
  const src = d.source as { center?: number; sigma?: number };
  return (
    <article className="finding" aria-labelledby="capa-h">
      <header className="finding-head">
        <h2 id="capa-h">
          <span className="mono">{c.reference}</span> — {c.title}
        </h2>
        <Badge group="capa.status" value={c.status} tone={statusTone(c.status)} />
        {c.reopened > 0 && <Badge group="capa" value={t("capa.reopenedN", { n: c.reopened })} tone="crit" />}
        <span className="muted">{tv("capa.source", c.source_kind)}</span>
      </header>
      <Notice msg={msg} error={err} />
      <section aria-labelledby="capa-src">
        <h3 id="capa-src">{t("capa.sourceEvidence")}</h3>
        {typeof src.center === "number" && typeof src.sigma === "number" && <p className="muted num">{t("capa.spcLimits", { center: src.center.toFixed(4), sigma: src.sigma.toFixed(4) })}</p>}
        <ul className="list">
          {d.source_links.map((l) => (
            <li key={keyOf(l)}>
              <EvidenceLabel id={keyOf(l)} info={d.evidence[keyOf(l)]} />
            </li>
          ))}
        </ul>
      </section>
      <section aria-labelledby="capa-steps">
        <h3 id="capa-steps">{t("capa.steps")}</h3>
        <p className="muted">{t("capa.stepsHint")}</p>
        <div role="tablist" aria-label={t("capa.steps")} className="actions">
          {d.steps.map((s, i) => (
            <button
              key={s.discipline}
              ref={(el) => {
                tabs.current[i] = el;
              }}
              role="tab"
              id={`capa-tab-${s.discipline}`}
              aria-selected={s.discipline === sel}
              aria-controls="capa-panel"
              tabIndex={s.discipline === sel ? 0 : -1}
              onClick={() => setSel(s.discipline)}
              onKeyDown={onKey}
            >
              {tv("capa.d", String(s.discipline))} <Badge group="capa.stepStatus" value={s.status} tone={s.status === "done" ? "ok" : "neutral"} />
            </button>
          ))}
        </div>
        <div role="tabpanel" id="capa-panel" aria-labelledby={`capa-tab-${sel}`} tabIndex={0} className="claim-review">
          {step.status === "done" ? (
            <p className="callout ok">
              {t("capa.locked")} {step.completed_at && <span className="muted num">{t("capa.doneBy", { at: fmtDate(step.completed_at) })}</span>}
            </p>
          ) : c.status !== "open" ? (
            <p className="callout">{t("capa.caseLocked")}</p>
          ) : null}
          <StepEditor d={d} step={step} draft={draft} setDraft={setDraft} editable={editable} />
          {sel === 5 && (
            <Actions
              d={d}
              editable={{ add: canWrite && c.status === "open" && d.steps[3].status === "done" && step.status === "open", done: canWrite && c.status === "open" && step.status === "done" }}
              onAdd={(b) => act("POST", `/capa/cases/${c.id}/actions`, b, t("capa.actionAdded"))}
              onDone={(id, ev, comment) => act("POST", `/capa/actions/${id}/done`, { evidence: ev, comment: comment || null }, t("capa.actionDone"))}
            />
          )}
          {step.status === "open" && (
            <section aria-labelledby="capa-miss">
              <h4 id="capa-miss">{t("capa.missing")}</h4>
              {step.missing.length === 0 ? (
                <p className="muted">{t("capa.ready")}</p>
              ) : (
                <ul className="list callout warn">
                  {step.missing.map((m) => (
                    <li key={m}>{t(`capa.missing.${m}` as never)}</li>
                  ))}
                </ul>
              )}
            </section>
          )}
          {editable && (
            <div className="actions">
              <button type="button" onClick={() => act("PUT", `/capa/cases/${c.id}/steps/${sel}`, { content: clean(draft) }, t("capa.saved"))}>
                {t("capa.save")}
              </button>
              {sel < 8 && (
                <button type="button" className="primary" onClick={() => act("POST", `/capa/cases/${c.id}/steps/${sel}/complete`, {}, t("capa.completed"))}>
                  {t("capa.complete")}
                </button>
              )}
              {sel === 8 && (
                <button type="button" className="primary" onClick={() => act("POST", `/capa/cases/${c.id}/closure/request`, {}, t("capa.closureRequested"))}>
                  {t("capa.requestClosure")}
                </button>
              )}
            </div>
          )}
          {canWrite && c.status === "open" && step.status === "done" && (
            <div className="actions">
              <label>
                {t("capa.reopenReason")}
                <input value={reason} onChange={(e) => setReason(e.target.value)} />
              </label>
              <button type="button" onClick={() => act("POST", `/capa/cases/${c.id}/steps/${sel}/reopen`, { comment: reason }, t("capa.reopened"))}>
                {t("capa.reopenStep")}
              </button>
            </div>
          )}
          {sel === 8 && c.status === "closure_pending" && can("review_decide") && (
            <section aria-labelledby="capa-close" className="review-box">
              <h4 id="capa-close">{t("capa.closure")}</h4>
              <p className="muted">{t("capa.fourEyes")}</p>
              <label className="stack">
                {t("capa.closureComment")}
                <textarea value={closeComment} onChange={(e) => setCloseComment(e.target.value)} />
              </label>
              <div className="actions">
                <button type="button" className="primary" disabled={c.closure_requested_by === me.user_id} onClick={() => act("POST", `/capa/cases/${c.id}/closure/approve`, { comment: closeComment || null }, t("capa.closed"))}>
                  {t("capa.approveClosure")}
                </button>
                <button type="button" onClick={() => act("POST", `/capa/cases/${c.id}/closure/reject`, { comment: closeComment }, t("capa.returned"))}>
                  {t("capa.rejectClosure")}
                </button>
              </div>
            </section>
          )}
        </div>
      </section>
      <Supplier d={d} canWrite={canWrite} canReview={can("review_decide")} act={act} />
      <section aria-labelledby="capa-hist">
        <h3 id="capa-hist">{t("capa.history")}</h3>
        <ol className="list">
          {d.events.map((e, i) => (
            <li key={i}>
              <span className="muted num">{fmtDate(e.at)}</span> {e.discipline ? `D${e.discipline} ` : ""}
              {tv("capa.event", e.kind)} {e.actor_kind === "system" ? <span className="muted">({t("capa.system")})</span> : <span className="muted">{d.users.find((u) => u.id === e.actor)?.name ?? ""}</span>}
            </li>
          ))}
        </ol>
      </section>
    </article>
  );
}

function OpenForm({ onOpened }: { onOpened: (d: CapaDetail) => void }) {
  const { api } = useSession();
  const { t, tv } = useI18n();
  const [kind, setKind] = useState<string>("conflict");
  const [id, setId] = useState("");
  const [st, setSt] = useState("part");
  const [sk, setSk] = useState("");
  const [pred, setPred] = useState("");
  const { msg, setMsg, err, guard } = useGuard();
  return (
    <form
      aria-label={t("capa.open")}
      onSubmit={(e) => {
        e.preventDefault();
        guard(async () => {
          const body = kind === "spc" ? { source_kind: kind, spc: { subject_type: st, subject_key: sk.trim(), predicate: pred.trim() } } : { source_kind: kind, source_id: id.trim() };
          const d = await api.request<CapaDetail>("POST", "/capa/cases", body);
          setMsg(t("capa.opened", { ref: d.case.reference }));
          onOpened(d);
        });
      }}
    >
      <div className="toolbar">
        <label>
          {t("capa.openFrom")}
          <select value={kind} onChange={(e) => setKind(e.target.value)}>
            {SOURCES.map((s) => (
              <option key={s} value={s}>
                {tv("capa.source", s)}
              </option>
            ))}
          </select>
        </label>
        {kind === "spc" ? (
          <>
            <label>
              {t("capa.subjectType")}
              <input value={st} onChange={(e) => setSt(e.target.value)} required />
            </label>
            <label>
              {t("capa.subjectKey")}
              <input value={sk} onChange={(e) => setSk(e.target.value)} required />
            </label>
            <label>
              {t("capa.predicate")}
              <input className="mono" value={pred} onChange={(e) => setPred(e.target.value)} required />
            </label>
          </>
        ) : (
          <label>
            {t("capa.sourceId")}
            <input className="mono" value={id} onChange={(e) => setId(e.target.value)} required />
          </label>
        )}
        <button type="submit" className="primary">
          {t("capa.open")}
        </button>
      </div>
      <Notice msg={msg} error={err} />
    </form>
  );
}

export function CapaView() {
  const { api, can } = useSession();
  const { t, tv, lang } = useI18n();
  const [status, setStatus] = useState("open");
  const list = useLoad(() => api.request<CapaCase[]>("GET", `/capa/cases?status=${status}`), [status]);
  const notes = useLoad(() => api.request<Notification[]>("GET", "/capa/notifications"), []);
  const [detail, setDetail] = useState<CapaDetail | null>(null);
  const { msg, setMsg, err, guard } = useGuard();
  const fetchDetail = async (id: string) => setDetail(await api.request<CapaDetail>("GET", `/capa/cases/${id}`));
  const unread = (notes.data ?? []).filter((n) => !n.read_at);
  return (
    <div>
      <PageTitle title={t("nav.capa")} lead={t("capa.lead")} />
      <div className="toolbar">
        <label>
          {t("findings.status")}
          <select value={status} onChange={(e) => setStatus(e.target.value)}>
            {STATUSES.map((s) => (
              <option key={s} value={s}>
                {tv("capa.status", s)}
              </option>
            ))}
          </select>
        </label>
        {can("decision_write") && (
          <button
            type="button"
            onClick={() =>
              guard(async () => {
                const r = await api.request<{ notifications_new: number }>("POST", "/capa/overdue/run", {});
                setMsg(t("capa.overdueRan", { n: r.notifications_new }));
                notes.reload();
              })
            }
          >
            {t("capa.runOverdue")}
          </button>
        )}
      </div>
      <Notice msg={msg} error={err ?? list.error} />
      {unread.length > 0 && (
        <Panel title={t("capa.notifications")} id="cn">
          <ul className="list">
            {unread.map((n) => (
              <li key={n.id}>
                <button className="link" type="button" onClick={() => guard(() => fetchDetail(n.case_id))}>
                  {t("capa.notification.action_overdue", { due: n.due_date, what: n.description, ref: n.reference })}
                </button>{" "}
                <button type="button" onClick={() => guard(async () => (await api.request("POST", `/capa/notifications/${n.id}/read`, {}), notes.reload()))}>
                  {t("capa.markRead")}
                </button>
              </li>
            ))}
          </ul>
        </Panel>
      )}
      {can("decision_write") && (
        <Panel title={t("capa.open")} id="co">
          <OpenForm onOpened={(d) => (setDetail(d), list.reload())} />
        </Panel>
      )}
      <div className="grid2 wide-right">
        <Panel title={t("capa.list")} id="c-list">
          <Loading on={list.loading} />
          {list.data && list.data.length === 0 && <Empty text={t("capa.none")} />}
          {list.data && list.data.length > 0 && (
            <DataTable
              label={t("capa.list")}
              rows={list.data}
              rowKey={(c) => c.id}
              selectedKey={detail?.case.id}
              onActivate={(c) => guard(() => fetchDetail(c.id))}
              cols={[
                {
                  key: "ref",
                  label: t("capa.reference"),
                  value: (c) => c.reference,
                  render: (c) => (
                    <button className="link mono" onClick={(e) => (e.stopPropagation(), guard(() => fetchDetail(c.id)))}>
                      {c.reference}
                    </button>
                  ),
                },
                { key: "title", label: t("capa.title"), value: (c) => c.title, render: (c) => <span className="clamp">{c.title}</span> },
                { key: "source", label: t("capa.openFrom"), value: (c) => c.source_kind, render: (c) => <Badge group="capa.source" value={c.source_kind} tone="info" /> },
                {
                  key: "progress",
                  label: t("capa.progress"),
                  value: (c) => c.progress,
                  render: (c) => (
                    <span className="num">
                      {t("capa.progressOf", { n: c.progress })} {c.overdue_actions > 0 && <Badge group="capa" value={t("capa.overdueN", { n: c.overdue_actions })} tone="crit" />}
                      {c.reopened > 0 && <Badge group="capa" value={t("capa.reopenedN", { n: c.reopened })} tone="crit" />}
                    </span>
                  ),
                },
                { key: "status", label: t("findings.status"), value: (c) => c.status, render: (c) => <Badge group="capa.status" value={c.status} tone={statusTone(c.status)} /> },
              ]}
            />
          )}
        </Panel>
        <div lang={lang}>{detail ? <CaseView key={detail.case.id} d={detail} onChange={(d) => (setDetail(d), list.reload())} /> : <Empty text={t("capa.select")} />}</div>
      </div>
    </div>
  );
}
