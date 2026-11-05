# KIZUNA notes: how to build it, and how it applies to Chuo-Sogo

2026-11-05. Short working notes. Full plan: `PLAN.md` (comparison table, phases K0-K6, risks).

## What KIZUNA is, in one line
Every business decision keeps a link to the evidence it used. When evidence changes, only the affected decisions are
flagged, and anyone can check a decision was made correctly without seeing the raw data.

## How it applies to Chuo-Sogo (factory-side procurement + savings-share fee)
Chuo-Sogo's income is 50% of the price drop, for 36 months. Every yen billed rests on a chain of decisions:

```
factory purchase records ─► baseline price ("price paid before") ─► quotes ─► comparison ─► supplier choice
        ─► factory approval ─► order ─► invoice 3-way match ─► savings line ─► monthly fee invoice
```

KIZUNA makes that chain provable and self-checking:

| Chuo-Sogo situation | Without KIZUNA | With KIZUNA |
|---|---|---|
| Factory asks "why is this fee ¥120,000?" | Staff dig through records | Open the fee line: baseline, the purchase rows it came from, quote, order, invoice, the rule used, all linked; replay gives the same number |
| A purchase-history import was wrong and is withdrawn | Old baselines and fees stay as they were | Every baseline, comparison and savings line that used those rows is flagged "needs review" in the inbox; nothing is reversed automatically |
| Same supplier written 3 ways (half/full-width, kana) | Treated as 3 suppliers; "same supplier as before" column can be wrong | Supplier names are matched with scores; staff confirm the merge; the decision records which match it used |
| Two records disagree (two prices for one item and date, wrong unit, part revision changed) | One silently wins | Both kept, a conflict is recorded, and every decision that touched either one shows it |
| An AI agent proposed a credit note or a negotiation brief | Audit log entry | The proposal, the evidence it read, and the person who approved it are linked as one decision record |
| A factory auditor or the CEO wants proof without seeing supplier prices | Not possible | Export a proof bundle (hash commitments + Merkle proof anchored in the audit chain) checked offline by a small verify tool |

Why this matters commercially: billing disputes are the main risk of a savings-share model. A fee that can be explained
and re-computed on demand is a sales argument to a ¥10bn+ factory, and the same evidence trail is what a later
standalone KIZUNA product would be sold on.

## How to build it (order and rules)
1. **Inside Procure first**, as an "evidence and decision integrity" module. Separate product only after pilots ask.
   Keep the evidence code free of procurement types so it can be moved out later as a crate.
2. Phases (details in PLAN.md §3):
   - **K0** synthetic factory data with injected faults + ground truth; measure today's item matching as the baseline.
   - **K1** `evidence_items` (valid time + recorded time) and `decision_records` + links; capture inside the existing
     transactions (approvals, baselines, savings lines, discrepancy resolution, agent decisions, alias confirmation).
   - **K2** entity resolution for items and suppliers, human-confirmed merges. Target F1 ≥ 0.90.
   - **K3** applicability score (integer, with reasons) and conflict records. Target conflict recall ≥ 0.85.
   - **K4** selective invalidation: dependency graph, BFS, one inbox item per affected decision, p95 < 1 s.
   - **K5** stored derivations; replay must be 100% byte-identical.
   - **K6** hash commitments, daily Merkle root in the audit chain, offline verify CLI. ZK proofs and cross-company
     sharing are research, not MVP.
3. Rules that do not change: humans decide (KIZUNA flags and explains, never reverses an approval or a billed fee);
   one factory's data never feeds another's decisions; integer money and scores only; every phase passes
   `scripts/gate.sh full` and writes its measured numbers to `reports/kizuna/`.

## Do not claim yet
- The brief's patent numbers and the benchmark "Result" table are unverified/placeholder. Quote only numbers we measure.
- Palantir Foundry and Collibra already do lineage; Catena-X already does cross-company exchange. The novelty is the
  combination: conflict-preserving evidence + selective invalidation + replayable, verifiable decisions.

## Founder decisions before anything public
1. Patent/FTO with a patent attorney (弁理士) before any demo, paper or pitch describing K3-K6 (disclosure can kill novelty).
2. Trademark search for "KIZUNA" (very common word in Japan).
3. Feature name inside Procure vs separate product; pricing.
4. Any cross-company data sharing (changes the customer contract and Procure's data rule).

---

## 日本語メモ（要約）
**KIZUNA とは**：すべての判断に「根拠データ」を結び付けて残す仕組み。根拠が変われば、影響する判断だけに印が付き、生データを見せずに判断の正しさを第三者が確認できる。

**中央総合への適用**：報酬は値下がり額の50%（36か月）。報酬1円ごとに「以前の価格 → 見積 → 比較 → 仕入先選定 → 工場承認 → 発注 → 請求照合 → 削減額 → 月次請求」の判断の連鎖がある。KIZUNA により、
- 報酬の根拠をその場で示し、同じ計算を再実行して同じ金額になることを証明できる。
- 誤った購買記録を取り消すと、それを使った基準価格・比較・削減額だけが「要確認」になる（自動で取り消さない、判断は人）。
- 表記ゆれの仕入先（全角/半角/カナ）を照合し、担当者が確認して統合する。
- 矛盾するデータ（同じ品目・日付で2つの価格、単位違い、改訂番号違い）は両方残し、影響した判断に表示する。
- 仕入先の価格を見せずに、判断の正しさを証明するファイル（ハッシュ＋監査チェーン）を出せる。

**作り方**：まず Procure の中のモジュールとして K0〜K6 の順に作る（詳細は PLAN.md）。人が判断する原則、他社データを使わない原則は変えない。数値は実測したものだけを使う。

**社長・創業者の判断が必要なこと**：特許（弁理士に相談、公開前）、「KIZUNA」の商標調査、製品名と価格、他社間データ共有の可否。
