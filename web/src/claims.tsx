import { useEffect, useMemo, useState } from "react";
import { claimValue, confidence, sourceLocation } from "./format";
import { useI18n } from "./i18n";
import { useSession } from "./session";
import type { ArtifactDetail, Claim } from "./types";
import { Badge, Notice, short } from "./ui";
import { WhatIfPanel } from "./WhatIfPanel";

export const isAi = (c: Claim) => c.extraction_method.startsWith("ai:");

/** Model id of an AI-origin claim: provenance.ai.model, else parsed from `ai:<model>+<mapper>`. */
export function aiModel(c: Claim): string | null {
  if (!isAi(c)) return null;
  const p = c.provenance?.ai as { model?: unknown } | undefined;
  if (typeof p?.model === "string") return p.model;
  return c.extraction_method.slice(3).split("+")[0] || null;
}

/** Claim id: short form with the full id on hover and a copy button (copies the full id). */
export function ClaimId({ id }: { id: string }) {
  const { t } = useI18n();
  const [copied, setCopied] = useState(false);
  return (
    <span className="claim-id">
      <code title={id}>{short(id)}</code>
      <button
        type="button"
        className="icon-btn"
        aria-label={t("claim.copyId", { id: short(id) })}
        title={copied ? t("account.copied") : t("claim.copy")}
        onClick={(e) => {
          e.stopPropagation();
          navigator.clipboard?.writeText(id).then(
            () => setCopied(true),
            () => setCopied(false),
          );
        }}
      >
        {copied ? "✓" : "⧉"}
      </button>
    </span>
  );
}

export function AiBadge({ c }: { c: Claim }) {
  const { t } = useI18n();
  if (!isAi(c)) return null;
  return (
    <span className="badge warn" title={t("claim.aiModel", { model: aiModel(c) ?? "?" })}>
      {t("claim.aiOrigin")}
    </span>
  );
}

/**
 * Review of one claim. Mirrors the server rules so the reviewer sees them before acting (the server stays the
 * authority): nobody reviews evidence they entered or uploaded (four-eyes); approve/reject need a reason; a rejected
 * claim is final (correct by supersession); AI suggestions stay suggestions until accepted or rejected.
 */
export function ClaimReview({ claim, onReviewed, onClose }: { claim: Claim; onReviewed: (c: Claim) => void; onClose?: () => void }) {
  const { api, me, can } = useSession();
  const { t, tv, fmtDate } = useI18n();
  const [comment, setComment] = useState("");
  const [needComment, setNeedComment] = useState(false);
  const [owners, setOwners] = useState<string[] | null>(null);
  const [msg, setMsg] = useState("");
  const [err, setErr] = useState<unknown>(null);
  const [busy, setBusy] = useState(false);
  const [whatif, setWhatif] = useState(false);
  const c = claim;
  const ai = isAi(c);

  useEffect(() => {
    let live = true;
    setOwners(null);
    setWhatif(false);
    setComment("");
    setMsg("");
    setErr(null);
    const base = c.recorded_by ? [c.recorded_by] : [];
    if (!c.artifact_id) return setOwners(base);
    api
      .request<ArtifactDetail>("GET", `/artifacts/${c.artifact_id}`)
      .then((d) => live && setOwners([...base, ...(d.artifact.created_by ? [d.artifact.created_by] : []), ...d.captures.map((x) => x.uploaded_by)]))
      .catch(() => live && setOwners(base));
    return () => {
      live = false;
    };
  }, [api, c.id, c.artifact_id, c.recorded_by]);

  const own = owners?.includes(me.user_id) ?? false;
  const blocked = c.superseded_at ? "superseded" : c.review_status === "rejected" ? "rejected" : !can("review_decide") ? "role" : own ? "own" : null;

  async function submit(decision: "approve" | "reject" | "request_evidence") {
    if (decision !== "request_evidence" && !comment.trim()) return setNeedComment(true);
    setNeedComment(false);
    setErr(null);
    setMsg("");
    setBusy(true);
    try {
      const out = await api.request<Claim>("POST", `/claims/${c.id}/review`, { decision, comment: comment.trim() || null });
      setComment("");
      setMsg(t("claim.reviewed", { status: tv("review", out.review_status) }));
      onReviewed(out);
    } catch (x) {
      setErr(x);
    } finally {
      setBusy(false);
    }
  }

  return (
    <article className="claim-review" aria-labelledby={`cr-${c.id}`}>
      <header className="finding-head">
        <h3 id={`cr-${c.id}`}>
          {t("claim.reviewTitle")} — <span className="mono">{c.subject_key}</span> <span className="mono">{c.predicate}</span> = <b className="mono">{claimValue(c)}</b>
        </h3>
        <Badge group="review" value={c.review_status} />
        <AiBadge c={c} />
        {!c.superseded_at && c.review_status !== "rejected" && (
          <button type="button" aria-expanded={whatif} onClick={() => setWhatif((w) => !w)}>
            {t("whatif.open")}
          </button>
        )}
        {onClose && (
          <button type="button" className="ghost" onClick={onClose}>
            {t("claim.close")}
          </button>
        )}
      </header>
      <dl className="kv">
        <dt>{t("claim.id")}</dt>
        <dd>
          <ClaimId id={c.id} />
        </dd>
        <dt>{t("finding.document")}</dt>
        <dd>
          {c.artifact_id ? (c.artifact_display_name ?? short(c.artifact_id)) : t("evidence.manual")} · {sourceLocation(t, c.source_location)}
        </dd>
        <dt>{t("evidence.method")}</dt>
        <dd>{ai ? t("claim.aiMethod") : tv("method", c.extraction_method)}</dd>
        {ai && (
          <>
            <dt>{t("claim.model")}</dt>
            <dd className="mono">{aiModel(c)}</dd>
          </>
        )}
        <dt>{t("finding.confidence")}</dt>
        <dd className="num">{confidence(t, c.confidence_bps)}</dd>
        <dt>{t("claim.recorded")}</dt>
        <dd className="num">{fmtDate(c.recorded_at)}</dd>
      </dl>
      {ai && <p className="callout warn">{t("claim.aiNote")}</p>}
      {whatif && <WhatIfPanel key={c.id} claim={c} onClose={() => setWhatif(false)} />}
      <Notice msg={msg} error={err} />
      {blocked ? (
        <p className={blocked === "own" ? "callout crit" : "callout"} role="note">
          {t(`claim.blocked.${blocked}` as never)}
        </p>
      ) : owners === null ? (
        <p className="muted" aria-live="polite">
          {t("common.loading")}
        </p>
      ) : (
        <section className="review-box" aria-labelledby={`cra-${c.id}`}>
          <h4 id={`cra-${c.id}`}>{t("finding.recordReview")}</h4>
          <p className="muted">{t("claim.fourEyes")}</p>
          <label className="stack">
            {t("finding.comment")}
            <textarea value={comment} maxLength={2000} onChange={(e) => setComment(e.target.value)} aria-invalid={needComment} aria-describedby={`crh-${c.id}`} />
          </label>
          <p id={`crh-${c.id}`} className={needComment ? "notice crit" : "muted"} role={needComment ? "alert" : undefined}>
            {t("finding.commentHint")}
          </p>
          <div className="actions">
            <button disabled={busy} onClick={() => submit("approve")}>
              {t(ai ? "claim.acceptAi" : "claim.accept")}
            </button>
            <button disabled={busy} className="danger" onClick={() => submit("reject")}>
              {t(ai ? "claim.rejectAi" : "claim.reject")}
            </button>
            <button disabled={busy} onClick={() => submit("request_evidence")}>
              {t("finding.requestEvidence")}
            </button>
          </div>
          {ai && <p className="hint">{t("claim.aiRequestNote")}</p>}
        </section>
      )}
    </article>
  );
}

/** Searchable, keyboard-operable claim picker for decision evidence (current claims of one record). */
export function ClaimPicker({ subjectType, subjectKey, selected, onChange }: { subjectType: string; subjectKey: string; selected: string[]; onChange: (ids: string[]) => void }) {
  const { api } = useSession();
  const { t, tv } = useI18n();
  const [claims, setClaims] = useState<Claim[] | null>(null);
  const [err, setErr] = useState<unknown>(null);
  const [q, setQ] = useState("");
  const key = subjectKey.trim();
  useEffect(() => {
    setClaims(null);
    setErr(null);
    if (!key) return;
    let live = true;
    const h = setTimeout(() => {
      api
        .request<Claim[]>("GET", `/claims?subject_type=${encodeURIComponent(subjectType)}&subject_key=${encodeURIComponent(key)}&limit=500`)
        .then((r) => live && setClaims(r))
        .catch((x) => live && setErr(x));
    }, 250);
    return () => {
      live = false;
      clearTimeout(h);
    };
  }, [api, subjectType, key]);
  const shown = useMemo(() => {
    const f = q.trim().toLowerCase();
    return (claims ?? []).filter(
      (c) => !f || [c.id, c.predicate, claimValue(c), c.artifact_display_name ?? "", c.revision ?? ""].some((s) => s.toLowerCase().includes(f)),
    );
  }, [claims, q]);
  const toggle = (id: string) => onChange(selected.includes(id) ? selected.filter((x) => x !== id) : [...selected, id]);
  return (
    <fieldset className="claim-picker">
      <legend>{t("decisions.evidenceIds")}</legend>
      {!key ? (
        <p className="muted">{t("picker.needKey")}</p>
      ) : (
        <>
          <label>
            {t("picker.search")}
            <input type="search" value={q} onChange={(e) => setQ(e.target.value)} aria-controls="picker-list" />
          </label>
          <Notice error={err} />
          {claims === null && !err && <p className="muted">{t("common.loading")}</p>}
          {claims && claims.length === 0 && <p className="muted">{t("picker.none", { key })}</p>}
          <ul id="picker-list" className="picker-list" aria-label={t("picker.results", { n: shown.length })}>
            {shown.map((c) => (
              <li key={c.id}>
                <label className="check">
                  <input type="checkbox" checked={selected.includes(c.id)} onChange={() => toggle(c.id)} />
                  <span className="mono">{c.predicate}</span> = <b className="mono">{claimValue(c)}</b>
                  {c.revision && <span className="badge info">{t("evidence.revShort", { rev: c.revision })}</span>}
                  <span className="badge neutral">{tv("review", c.review_status)}</span>
                  {isAi(c) && <span className="badge warn">{t("claim.aiOrigin")}</span>}
                  <span className="muted">{c.artifact_display_name ?? t("evidence.manual")}</span>
                  <code title={c.id}>{short(c.id)}</code>
                </label>
              </li>
            ))}
          </ul>
        </>
      )}
      <p className="hint" aria-live="polite">
        {t("picker.selected", { n: selected.length })}
      </p>
    </fieldset>
  );
}
