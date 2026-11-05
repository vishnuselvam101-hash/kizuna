# Glossary

Terms as they appear in the English and Japanese UI. Use the same words in reports and training.

| English (UI) | 日本語 (UI) | Meaning |
|---|---|---|
| Evidence | 証拠 | Documents and the claims taken from them. |
| Claim | クレーム | One stated fact from one source: identifier, attribute, value, unit, revision, source location. |
| Attribute | 属性 | What a claim is about (dimension, material, approval…). |
| Source | 出典 | The document and location a claim comes from. |
| Source location | 位置 | Page, sheet, cell, row, column or line inside the source. |
| Manual entry | 手入力 | A claim typed in by a person, with no source document. |
| Confidence | 信頼度 | Reliability of the extraction method. Not the probability that the value is true. |
| Review | レビュー | A human judgement recorded with a reason. |
| Unreviewed / Accepted / Rejected / Needs evidence | 未レビュー／承認済み／却下／証拠待ち | Review states of a claim. |
| Superseded | 置換済み | Replaced by a newer version; the old one is kept. |
| Document | 文書 | An uploaded file, stored unchanged. |
| Capture | 取込 | One upload event of a file; identical files share storage but each capture is recorded. |
| Accepted / Quarantined / Rejected (document) | 受理／隔離／拒否 | Scan outcome of a document. |
| Integrity | 完全性 | The stored bytes are unchanged since capture. |
| Extract / Re-extract | クレーム抽出／再抽出 | Turn a document into unreviewed claims / run it again with a reason. |
| Record | レコード | A component or supplier and its claims. |
| Component / Supplier | 部品／サプライヤー | Record types. |
| Identifier | 識別子 | Part number or supplier name that keys a record. |
| Revision | リビジョン | The version of a document or specification. |
| Conflict | 矛盾 | A contradiction or gap found in the evidence; a prompt for review, not a verdict. |
| Run verification | 検証を実行 | Re-run conflict detection over current claims. |
| Approve (documented waiver) | 特例として承認 | Accept a conflict as a recorded exception. |
| Reject evidence | 証拠を却下 | Mark the evidence behind a conflict as wrong. |
| Request more evidence | 追加証拠を依頼 | Keep a conflict open until more evidence arrives. |
| No longer applies | 対象外 | A conflict that disappeared after the evidence changed. |
| Decision | 判断 | A recorded outcome with the exact claims it relied on. |
| Rationale | 根拠 | The written basis of a decision. |
| Derivation hash | 導出ハッシュ | Hash over a decision and its evidence. |
| Valid / Potentially affected / Superseded / Revoked | 有効／影響の可能性／置換済み／取消済み | Decision statuses. |
| Affected decisions | 影響を受ける判断 | Decisions whose evidence changed after they were made. |
| Reconfirm | 再確認 | Record a new version of a decision on current evidence. |
| Revoke | 取り消す（取消） | End a decision for good. Cannot be undone. |
| Upstream decision | 上流の判断 | A decision another decision depends on. |
| Material / Not material | 重要／軽微 | Whether a change matters to a decision. |
| Audit trail | 監査証跡 | The list of every recorded action. |
| Audit chain / Hash chain | 監査チェーン／ハッシュチェーン | Links each audit event to the previous one so changes are detectable. |
| Intact / Broken | 改ざんなし／破損 | Result of verifying the chain. |
| Incident | インシデント | A security event pattern that a detection rule flagged. |
| Contained | 封じ込め済み | An incident whose automatic actions have run. |
| False positive | 誤検知 | An incident that was not a real threat. |
| Detection thresholds | 検知しきい値 | Event counts that raise an incident. |
| Import settings | 取込設定 | How CSV, XLSX and PDF files are read. |
| AI-assisted extraction | AI 支援抽出 | Optional label mapping by an external AI provider. |
| Authenticator | 認証アプリ | The app that produces sign-in codes (TOTP). |
| Organisation | 組織 | Your company's separate data space in KIZUNA. |
| Administrator / Reviewer / Analyst / Auditor / Viewer | 管理者／レビュアー／アナリスト／監査担当／閲覧者 | Roles. |

Terms not shown in the UI:
| English | 日本語 | Meaning |
|---|---|---|
| Four-eyes principle | 4つの目の原則（ダブルチェック） | Nobody reviews or releases their own work. |
| Security key | セキュリティキー | WebAuthn hardware second factor (API only). |
| Recovery code | リカバリーコード | One-time code for a lost security key. |
| Operator | 運用者 | Whoever runs the server, backups and secrets. |
