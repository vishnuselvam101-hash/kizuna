# ADR 0027: Server-generated text as message codes (日本語 UI without English server text)

Status: accepted (2026-11-06). Context: the server wrote finished English sentences (finding explanations,
three-way-match and integrity/drift reasons, invalidation reasons, incident summaries, response-action details,
limitations, report statements). The Japanese UI showed them in English; only integrity signals had an ad-hoc
`why_ja`.

## Decision
- New crate `kizuna-messages`: `Msg { code, params }` (params: string, number, list, null or a nested `Msg`) and
  `Lang`. Engines emit a `Msg`; nothing composes user-facing sentences any more.
- One catalogue, `web/src/i18n-server.json` (code → `{en, ja}` with `{name}` placeholders), read by the server
  (`include_str!`) and imported by the web app (`msg.ts`, `useI18n().tm`). The Japanese is written for a quality
  engineer and uses the UI's terms (証拠, クレーム, 判断, 矛盾, リビジョン, 発注書, 成績書).
- Wire format is additive: every English field stays (`why`, `reason`, `summary`, `detail`, `limitations`,
  `statement`) as the plain fallback for API clients, next to `msg` / `*_msg`. Migration 0030 adds nullable
  `incidents.summary_msg`, `invalidation_events.reason_msg`, `response_actions.detail_msg`; JSON explanations
  carry `msg`. Rows recorded before this show their English text; stored review-batch impacts fall back to
  `text.verbatim`.
- Server-rendered artefacts take the request language: explicit `?lang=ja|en`, else `Accept-Language` (the web
  app sends its UI language), else the endpoint's previous default (Japanese for letters, English for reports and
  generated titles), so API clients sending neither see no change. This covers the verification report (JSON gains `lang`, `text`,
  `reason_text`, `statement_text`; Markdown is fully localised), the report inside the evidence pack, letter
  downloads without `lang`, and the title of a match-acceptance decision.
- Error codes were already stable; every `ApiError` code now has `err.<code>` text in both web dictionaries.

## Enforcement
`crates/kizuna-messages/tests/catalog.rs` fails when a `Msg::new("…")` code is missing from the catalogue, a
catalogue code is unused, an entry lacks EN or JA, JA drops an EN placeholder or contains an English sentence, or
an API error code lacks `err.<code>` in EN and JA. `kizuna-api/tests/i18n_messages.rs` seeds data and checks every
explanation, reason, incident summary and action detail carries a known code that renders as Japanese, and that
reports follow the request language. `web/src/i18n-server.test.tsx` and the Playwright JA journeys
(`noEnglishSentences`) check the Japanese screens show no English sentence for seeded data.

## Consequences
Adding server text means adding a catalogue entry (EN + JA) in the same change. Decision `notes` and audit
details stay English machine records; the offline verifier (`kizuna-verify-pack`) keeps its own bilingual text.
