# ER v2 typo-review tuning — 2026-11-05

Evidence label: **SIMULATED** data (synthetic, deterministic generators, including the evidence context), **MEASURED**
quality from one real run of `kizuna-er-compare` at commit be409ab (release build, 5 runs, clean worktree). Raw output with provenance:
`reports/er/er_typo_tuning.json`. Nothing below is typed by hand except the prose. Follows
[ER_V2_2026-11-05.md](ER_V2_2026-11-05.md).

## Problem

The v2 one-character rule sent every part-number pair one keyed character apart to review. On the dense synthetic
master that was 5 091 pairs for 301 true typos (11 700 keys); precision fell from 0.999 (rule off) to 0.688.

## Change

- **Context agreement (strict mode).** A typo-shaped pair goes to `requires_human_review` only when the evidence
  about the two keys agrees: same supplier **and** same drawing, or mean field agreement ≥ `context_floor`
  (default 0.6) over the fields both keys have — description/part name (token Jaccard; ASCII words, kana/kanji
  bigrams), supplier (folded company name), drawing (identifier without revision), material. A pair with **no
  comparable context** (no field on both sides) still goes to review (conservative). Otherwise the pair gets its
  ordinary Fellegi–Sunter outcome (in practice: not listed). Reasons: `possible_typo_or_different_part` +
  `same_supplier_and_drawing` / `context_agrees` / `no_comparable_context`.
- **Per-tenant mode** (`GET/PUT /api/v1/settings/entity-resolution`, `{expected_version, typo_review}`):
  `off` (no rule), `strict` (default), `broad` (every typo-shaped pair to review, the previous behaviour). Stored in
  the append-only `tenant_settings` (kind `entity_resolution`, migration 0022); change needs a tenant admin with MFA,
  is audited (`settings.entity_resolution_updated`) and raises `admin.settings_changed`, flagged `weakening` when the
  mode becomes less sensitive (broad → strict → off).
- **API context:** current part claims `*.part_name`, `*.supplier` (or `scope.supplier`), `*.drawing_number`,
  `*.material`; rejected and AI-suggested claims are ignored (context can keep a pair out of review).
- **Never auto-merge is unchanged:** context only decides review vs ordinary scoring; v2 still lists every non-safe
  pair for a human decision, in every mode.

## Datasets

- **Dense** `ja_parts` (unchanged keys and truth; checked by test): 20 prefixes, three formats, ~20 % one-digit
  siblings, 0–3 noisy forms per part.
- **Sparse** `ja_parts_sparse` (new): 12 prefixes, random 6-digit numbers (`BRK-123456`, `BRK123-456`), 5 %
  last-digit siblings, 0/1/2 noisy forms with p = 0.55/0.30/0.15 (same transforms, including rare keying typos).
  Sequentially allocated masters are denser than this; the two sets bracket the range, neither is measured.
- **Context** (`part_contexts`, both sets, own random stream): master record per part (description `<noun> <size>`
  EN or JA, one of 40 suppliers, own drawing `D-nnnnnn`, family material); a sibling keeps the parent's supplier
  p = 0.7, material p = 0.7, drawing p = 0.15, size p = 0.15; non-canonical forms are document occurrences showing
  description 0.5 / supplier 0.85 / drawing 0.6 / material 0.5, either language, second-source supplier p = 0.03.
  **These probabilities are assumptions, and the strict rule's results depend on them directly.**

"Typo pairs" = true pairs with a keying error or keying transposition on one side. "Review load" = pairs with
outcome `requires_human_review` (typo rule, ambiguous clusters, revision/legal-form conflicts).

## Results

| dataset | keys | mode | precision | recall | F1 | review load (true) | typo-rule pairs (true) | typos in review |
|---|---|---|---|---|---|---|---|---|
| dense 1 000 | 2 307 | v1 | 0.772 | 0.894 | 0.829 | 1 939 (1 413) | — | 37 / 75 |
| | | v2 off | 0.998 | 0.964 | 0.981 | 1 165 (1 162) | 0 | 0 / 75 |
| | | v2 broad (before) | 0.740 | 0.998 | 0.850 | 2 003 (1 283) | 787 (70) | 70 / 75 |
| | | **v2 strict** | 0.881 | 0.998 | 0.936 | 1 540 (1 263) | 344 (70) | **70 / 75** |
| dense 5 000 | 11 700 | v1 | 0.737 | 0.920 | 0.818 | 11 228 (7 795) | — | 173 / 330 |
| | | v2 off | 0.999 | 0.969 | 0.984 | 6 381 (6 374) | 0 | 0 / 330 |
| | | v2 broad (before) | 0.688 | 0.997 | 0.814 | 11 809 (7 012) | 5 091 (301) | 301 / 330 |
| | | **v2 strict** | 0.881 | 0.996 | 0.935 | 8 270 (6 846) | 1 705 (288) | **288 / 330** |
| sparse 1 000 | 1 544 | v1 | 0.872 | 0.949 | 0.909 | 383 (311) | — | 7 / 19 |
| | | v2 off | 1.000 | 0.972 | 0.986 | 224 (224) | 0 | 0 / 19 |
| | | v2 broad (before) | 0.858 | 1.000 | 0.923 | 379 (268) | 130 (19) | 19 / 19 |
| | | **v2 strict** | 0.963 | 1.000 | 0.981 | 279 (253) | 45 (19) | **19 / 19** |
| sparse 5 000 | 7 761 | v1 | 0.868 | 0.960 | 0.912 | 2 069 (1 663) | — | 31 / 94 |
| | | v2 off | 1.000 | 0.972 | 0.986 | 1 266 (1 266) | 0 | 0 / 94 |
| | | v2 broad (before) | 0.855 | 0.999 | 0.921 | 2 051 (1 479) | 661 (89) | 89 / 94 |
| | | **v2 strict** | 0.953 | 0.998 | 0.975 | 1 573 (1 406) | 253 (86) | **86 / 94** |

v1 also *lists* some typos as possible matches (40/75, 190/330, 10/19, 61/94) without forcing review. The E2E sets
contain no typo-shaped pairs: all modes are identical there (P = R = 1). Suppliers are unaffected (parts only).

Floor sweep (strict, typo-rule pairs / typos in review), dense 5 000: 0.4 → 2 875 / 295, 0.5 → 2 726 / 293,
0.6 → 1 705 / 288, 0.8 → 1 191 / 256; sparse 5 000: 0.4 → 415 / 88, 0.5 → 393 / 88, 0.6 → 253 / 86,
0.8 → 152 / 85 (all in the JSON under `typo_modes`).

## Reading the results

- **Strict vs broad (v2 before):** typo-rule pairs −67 % dense (5 091 → 1 705) and −62 % sparse (661 → 253);
  total review load −30 % / −23 %; typo recall into review 91.2 % → 87.3 % dense and 94.7 % → 91.5 % sparse
  (13 and 3 typos lost: their synthetic document context did not reach the floor and they were not
  surfaced at all). Precision 0.688 → 0.881 and F1 0.814 → 0.935 on the large dense set.
- **Off** has the best F1 (0.984 / 0.986) because F1 ignores *which* pairs are missed: it lists none of the typos.
  F1 is not the right objective for this rule; typo recall into review vs review load is.
- **Floor 0.6** was set a priori (a sibling sharing supplier and material but not drawing or size scores ≈ 0.58);
  0.8 halves the load again but loses more typos (256/330 dense). Not tuned on these sets.
- **Sparse** master: broad is already cheap (661 rule pairs for 7 761 keys); strict still removes most of it.
- Timings in the JSON are noisy (shared host, load average ≈ 21); strict costs one context preparation per key.

## Limits

- Everything depends on synthetic context assumptions; on a real master the agreement rates (how often a
  document shows supplier + drawing for a part) decide the trade-off. Calibrate the floor on a real part master.
- Context can only *suppress* a typo review; a wrong supplier or drawing in a document can hide a real typo
  (this is the 13/330). Tenants that cannot accept that should use `broad`.

## Kanji → reading dictionary: licence check (no dictionary added)

| source | content | licence | fit |
|---|---|---|---|
| SudachiDict (Works Applications) | morphological dictionary with katakana readings; *full* adds proper nouns | Apache-2.0 for all files; bundles UniDic (BSD-style notice) and part of NEologd (no separate licence named in LEGAL) | **best candidate**: permissive, commercial use, keep NOTICE; check NEologd-derived entries |
| UniDic (NINJAL) | readings, current releases | triple GPL-2 / LGPL-2.1 / BSD-3 (older 1.3.x terms were restrictive) | permissive under BSD-3; verify the exact release |
| mecab-ipadic | readings | ICOT/IPA BSD-like notice (non-OSI, historical redistribution concerns) | usable but weaker footing |
| JMdict / JMnedict (ENAMDICT) / KANJIDIC2 (EDRDG) | words, **names incl. company names**, kanji readings | CC BY-SA 4.0, commercial use allowed | share-alike on derived data: only as a separate, attributed data file |
| 法人番号公表サイト (National Tax Agency) | official corporate names **with フリガナ** (published since 2018-04) | 公共データ利用規約 1.0 based terms (attribution; CC BY 4.0 compatibility not confirmed in this check) | most relevant for supplier names; confirm terms before use |

Sources: SudachiDict `LEGAL` (github.com/WorksApplications/SudachiDict), PyPI SudachiDict pages, EDRDG licence page
(edrdg.org/edrdg/licence.html), NTA corporate-number site terms (houjin-bangou.nta.go.jp), AWS Open Data
"cotonoha-dic" registry entry and Guix packaging notes for UniDic/IPAdic. These are licence summaries, not legal
advice. As in the v2 report, a reading match should raise a pair to review, never to match.

## Reproduce

```
for c in "7 24" "11 48" "13 400"; do set -- $c; python3 scripts/e2e/gen_dataset.py --seed $1 --parts $2 --out /tmp/e2e_$1; done
cargo build --release -p kizuna-bench --bin kizuna-er-compare
target/release/kizuna-er-compare --runs 5 --e2e e2e_seed7_24=/tmp/e2e_7/ground_truth.json \
  --e2e e2e_seed11_48=/tmp/e2e_11/ground_truth.json --e2e e2e_seed13_400=/tmp/e2e_13/ground_truth.json \
  --out reports/er/er_typo_tuning.json
```
