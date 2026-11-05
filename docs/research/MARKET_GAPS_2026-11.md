# KIZUNA Verify: market, pain and gap research (2026-11)

Status: desk research, 2026-11-05. Extends `research-register.md` (new rows R7 to R12 proposed in section 6) and
`unverified-claims.md` (new rows U9 to U12 proposed in section 7). It does not replace them.

Rules followed:
- Every fact has a URL. All URLs were accessed on **2026-11-05** (written `[acc 11-05]` below only where a section mixes in other dates).
- Source quality is labelled: **P** = primary (regulator, government, vendor's own price page or press release, court or official report);
  **V** = a vendor marketing page about its own product; **A** = aggregator, review site or blog (often inconsistent); **N** = news.
- "Absence" findings (no one does X) are **UNVERIFIED**. They mean "not found in public material in this search", not "does not exist".
- No market sizes are given. **Market size for Japan and India: unknown** (U7 stays open).

---

## 1. Competitors and adjacent tools

### 1.1 Supplier quality, PPAP/FAI, QMS
| Product | What it does (relevant part) | Public price | Src |
|---|---|---|---|
| 1factory | FAI (AS9102), control plans, CMM import, supplier collaboration | QC $75/user/mo (min 5); supplier collaboration **$20 per connected supplier/mo** (min 5); QMS $20/user/mo (min 10); annual term | P https://1factory.com/pricing.html |
| QT9 QMS | general QMS (CAPA, docs, supplier) | not published; third parties disagree ($30 to $120/user/mo; ~$2,200/concurrent user/yr quoted early 2025) | A https://www.itqlick.com/qt9-qms/pricing, https://softwareconnect.com/go/qt9-qms-review-fbffa |
| APQP/PPAP Manager | PPAP document authoring | "$8,000 per user, one time" (Capterra) | A https://www.capterra.com/p/246226/APQPPPAP-Manager/ |
| SafetyCulture (PPAP app) | checklist-style PPAP | $24/user/mo, free tier to 10 users | V https://safetyculture.com/app/ppap-software |
| ETQ, Intelex, Siemens Opcenter Quality, ComplianceQuest | enterprise QMS incl. supplier quality | quote only (no public price found) | UNVERIFIED |
| Arena PLM (PTC) | ECO/ECR change management, supplier portal; SQM in enterprise tier | not published; aggregators range $73/user/mo to $3,000/user/yr | A https://www.selecthub.com/plm-software/arena-plm/, https://www.softwareadvice.com/manufacturing/arena-plm-profile/vs/ |

### 1.2 Japan: drawings (図面管理), mill sheets (ミルシート), inspection reports (検査成績書), OCR
| Product | What it does | Public price | Src |
|---|---|---|---|
| CADDi Drawer (キャディ) | AI drawing analysis, similar-drawing search, links related data by drawing attributes; consultant-led rollout | not published (quote by scope) | V https://caddi.com/th/drawer/ ; A https://www.aspicjapan.org/asu/service/34885 |
| AiZumen (町工場向け) | drawing management for job shops | Lite ¥10,000/mo, Plus ¥30,000, Pro ¥50,000, no setup fee (as of 2026-08, per a vendor column) | A https://www.issoh.co.jp/column/details/16736/ |
| 図面バンク | cloud drawing management | from ¥48,000/mo (comparison article) | A https://renue.co.jp/posts/drawing-management-system-comparison-2026-features-price-ai |
| ズメーン | drawing management | "要問合せ" | A https://www.itreview.jp/products/zumain/price |
| MILL.NET (東計電算 / MJS) | cloud mill-sheet store; AI-OCR per item, auto rename/register; claims 40+ mill layouts read | not found | V https://mono.ipros.com/product/detail/2000989722 |
| Mill-Box (三菱商事 → Mill-Box株式会社, est. 2026-08-01, MC 80%) | e-mill-sheet management, match to rebar tags, quantity/size totals, inspection-document support; 300 to 400+ user companies (figures differ by page) | not found | V https://prtimes.jp/main/html/rd/p/000000006.000173555.html ; N https://www.logi-today.com/983878 |
| Anlab "Inspection Sheet OCR" (2026-05) | reads handwriting and 印影, compares to limits, pass/fail | not found | N (aggregator) https://aeo.washinmura.jp/ai/%E6%A0%AA%E5%BC%8F%E4%BC%9A%E7%A4%BE%E3%82%A8%E3%83%BC%E3%82%A8%E3%83%8C%E3%83%A9%E3%83%9C/en/news/2026-05-19-anlab-launches-inspection-sheet-ocr-to-a?lang=en |
| カミナシ | digital shop-floor forms; claims 400 h/yr saved on inspection documents (vendor case) | not found | V https://pr.mono.ipros.com/kaminashi/product/detail/2001532619 |
| DX Suite (AI inside) | general AI-OCR incl. FAX/handwriting | inconsistent: ¥19,800/mo, ¥40,000~/mo, or ¥100,000/mo + ¥200,000 setup (by site) | A https://boxil.jp/service/6886/, https://saas.imitsu.jp/cate-ocr/service/3821/price, https://aismiley.co.jp/product/dx-suite/ |

### 1.3 Document AI for manufacturing (global)
| Product | What | Price | Src |
|---|---|---|---|
| Werk24 | drawing → JSON (title block, revision table, tolerances, GD&T, threads, roughness); customer claims 98% on price-driving features | not found; free trial 100 drawings | V https://werk24.io/blog/read-title-block-intelligently, https://werk24.io/case-studies/laserhub, https://werk24.io/llms.txt |
| Clappia (MTR validation app) | extract chemistry from MTC, compare to grade spec, pass/fail | not found | V https://www.clappia.com/blog/ai-app-for-steel-alloy-test-report-validation |
| Algoscale DocumentIQ | MTC field extraction | not found | V https://documentiq.algoscale.com/extract/mill-test-certificates |

### 1.4 Cross-company exchange, DPP, compliance declarations
| Product / scheme | What | Src |
|---|---|---|
| Catena-X ↔ Ouranos Ecosystem (JP) | 2025-03 interoperability PoC; first use case battery carbon-footprint exchange; plan to extend to quality management and compliance | P https://www.ipa.go.jp/en/pressrelease/press20250331.html ; P https://catena-x.net/news/catena-x-and-ouranos-ecosystem-successfully-demonstrate-data-space-interoperability/ |
| METI Ouranos project certification | METI says Ouranos is "still in its early stage"; 2025-05 selected auto/battery CFP + due-diligence sharing project | P https://www.meti.go.jp/english/press/2025/0509_002.html |
| Catena-X SME path | SMEs as tenants via EDC connectors, CSV upload, open-source SaaS (slide level only) | P https://detecon.com/drupal/sites/default/files/2024-06/Presentation_Dr._J%C3%BCrgen_Padberg_Catena-X_Sustainability_ATZ_Chemnitz_2024-05-15.pdf |
| Circularise "Smart Questioning" | ZK proofs: verifier asks range/set-membership questions against a supplier's committed secret data; Ethereum-based, patent pending | V https://www.circularise.com/blogs/smart-questioning-achieve-supply-chain-visibility-without-risking-data-privacy/ ; N https://springwise.com/supply-chain/innovation-manufacturing-digital-product-passports |
| chemSHERPA (JAMP/METI) | standard format for product chemical content; introduced because many survey sheets burdened suppliers | P https://www.meti.go.jp/policy/chemical_management/other/160301chemSHERPA.pdf |
| Assent, Source Intelligence, Siemens (Green) compliance | supplier declaration collection (RoHS/REACH/conflict minerals) | not checked in this pass: UNVERIFIED |

---

## 2. Customer pain evidence

### 2.1 Quality data falsification (Japan, and global analogues)
- Kobe Steel (2017): affected customers revised from ~200 to 500+; final investigation report put it at **688**; some practices back to the 1970s. N https://www.cbsnews.com/news/japans-kobe-steel-rocked-by-a-quality-scandal/ ; N https://erai.com/quarterly_newsletter/distribution/2018/q2/ (688 figure as reported; primary report not opened: verify).
- Panasonic Industry (2024-01): 93 cases of certification/contract violations over a long period; Mitsubishi Paper subsidiary (2024-05): falsified inspection data and skipped inspections. N https://monoist.itmedia.co.jp/mn/articles/2501/06/news066.html ; P https://www2.jpx.co.jp/disc/38640/140120250514549558.pdf
- MONOist 2025 outlook: 2024 saw "芋づる式" chains of fraud (one case triggers re-inspection at related firms); remedy direction = "人を介さない" data capture. N same URL.
- Auto type-approval (型式指定): Daihatsu 2023-12 (MLIT confirmed 142 cases / 46 models after inspection); MLIT asked 85 makers to self-check, **5 reported misconduct** (Toyota, Mazda, Yamaha, Honda, Suzuki), 80 none (2024). P https://www.mlit.go.jp/report/press/jidosha08_hh_005206.html ; N https://monoist.itmedia.co.jp/mn/spv/2401/17/news075.html
- JIS mark revocations for falsified records: 西部生コン 2024-09 (fake delivery slips, different mix shipped, QC records altered); ベンカン機工 2023-09; 畿北アサノコンクリート 2023-04; 日本軽金属 2021 (25 years of non-JIS test method). P https://www.kanto.meti.go.jp/press/data/20240918press_jis_torikeshihoukoku.pdf ; P https://www.kansai.meti.go.jp/5gisin/jis/20230929_pressrelease.pdf ; P https://www.meti.go.jp/press/2023/04/20230411001/20230411001.html ; N https://monoist.itmedia.co.jp/mn/articles/2107/06/news048.html
- KPMG Japan: quality fraud is not limited to large/listed firms. A https://kpmg.com/jp/ja/home/insights/2024/11/ki-qualityfraud-prevention.html
- METI "製造業の品質保証体制の強化に向けて" (2017-12) framed QA as a management issue after 2017 scandals. P https://www.meti.go.jp/report/whitepaper/mono/2018/honbun/mobile/honbun_mb/101025_mb.html
- Global MTR fraud: Bradken foundry metallurgist falsified results for ≥240 heats of Navy sub steel 1985–2017 (guilty plea); O'Neal/Leeco jury award $117M over fraudulent CoCs and MTRs; NZ Timber King forged test certificate. N https://www.oversight.gov/former-metallurgist-lab-director-pleads-guilty-major-fraud-usa ; N https://www.steelmarketupdate.com/2014/06/26/o-neal-and-leeco-steel-win-compensatory-punitive-damages/ ; N https://m.scoop.co.nz/stories/BU1804/S00656/timber-king-nz-steel-distributor-ordered-to-pay-401k.htm
- 2025 allegation (unresolved): transshipped plate with MTCs lacking required signatures/stamps. N https://www.steelmarketupdate.com/2025/11/25/alleged-transshipment-of-chinese-steel-plate-raises-red-flags-in-houston

### 2.2 Paper, FAX, re-keying (Japan)
- 中小企業庁 R3 取引条件改善状況調査: 48.5% of order-receiving SMEs handled electronic ordering (2021); FAX/phone remain. P https://www.chusho.meti.go.jp/keiei/gijut/digitalization/index.html
- JRI estimate (2021): B2B ordering ~30%+ digital by value, invoices a few %. A/N https://www.jri.co.jp/page.jsp?id=38211
- 中小企業庁 material: digitising order slips alone is not enough benefit to move off paper/FAX. P https://www.chusho.meti.go.jp/keiei/gijut/digitalization/download/system_r3.pdf
- Inspection reports: per-customer formats and re-typing measured values into Excel templates; pass/fail judgement near limits varies by person. A https://www.issoh.co.jp/column/details/16895/
- 2025 SME white paper (via blog): firms with no digitalisation fell 30.8% (2023) → 12.5% (2024). A https://blogs.itmedia.co.jp/business20/2025/05/dx_66.html (white paper itself not opened: verify)
- SME QMS adoption advice: start with inspection records + nonconformance; ~¥500k first year for 5 users (vendor column); IT subsidy covers up to 2 years of cloud fees. A https://www.issoh.co.jp/column/details/18583/

### 2.3 Supplier-side burden (declarations, certificates)
- chemSHERPA exists because many survey sheets burdened suppliers; METI says it reduces burden for both sides. P https://www.meti.go.jp/policy/chemical_management/other/160301chemSHERPA.pdf
- SME chemical-content response cost: ~¥2.23M/yr average, ~¥1.28M of it analysis (from a METI-linked deck; survey year/sample unknown: UNVERIFIED). P https://www.meti.go.jp/policy/chemical_management/other/douga_gaiyou_CN.pdf
- COCIR: substance declaration gathering is time-consuming with low response rates. P https://www.cocir.org/fileadmin/5.5_Policies_Environment/bomcheck_faq-june2008.pdf (2008, old)
- CoCs citing drawing without revision, wrong revision/batch, post-dated CoCs as red flags. A https://www.fictiv.com/articles/certificate-of-conformance

### 2.4 India
- BIS Quality Control Orders grew from <100 to ~800 products; separate licence per product and factory; 6–9 months; screw test example $605/sample; Gauba committee (2025-08) proposed relief for 200+ products; JETRO: 71.9% of Japanese manufacturers in India affected or expect impact. N https://www.outlookbusiness.com/economy-and-policy/how-quality-testing-norms-are-squeezing-indian-msmes ; N https://knnindia.co.in/news/newsdetails/global/bis-certification-burden-on-japanese-firms-signals-need-for-wider-qco-review-gtri (advocacy-leaning sources; official BIS/DPIIT texts not opened)
- Crisil-linked survey: 41% of MSME respondents keep billing/inventory records manually (42% of respondents manufacturing). A https://dqindia.com/features/how-indias-msmes-are-embracing-tech-and-whats-holding-them-back-9648759
- WhatsApp use 97% among surveyed MSMEs (PayNearby 2024). A https://www.myvi.in/business/enterprise-segments/smb/msme-growth-insights-study-report
- Quality-documentation practice in Indian MSME manufacturing specifically: **unknown** (ICRIER 2025 survey not read: https://icrier.org/pdf/Annual-Survey-MSMEs_India_2025.pdf).

### 2.5 Regulatory drivers
| Driver | Fact | Src |
|---|---|---|
| EU Battery Regulation passport | mandatory from 2027-02-18 for EV, LMT, industrial >2 kWh (scope details differ by source); due diligence moved to 2027-08 | P https://single-market-economy.ec.europa.eu/batteries_en ; A https://www.arianee.com/en/blog/battery-passport-timeline-2027-2036 |
| ESPR wider DPP | timeline not verified in this pass | UNVERIFIED |
| CBAM | definitive period from 2026-01-01; first annual declaration for 2026 imports due 2027-09-30; default values marked up (India steel +10% 2026 → +30% 2028 per one source); exporters face commercial pressure to supply verified data | A https://greensutra.in/news/cbam-explained-2026/ ; A https://globaldeal.io/finder/articles/japan-to-eu-cbam-2026-compliance-guide ; regulation text not opened: verify |
| ISO 9001:2026 | FDIS ballot 2026; publication expected 2026-09; status "under publication" per one source; transition terms unconfirmed | A https://www.lrqa.com/en/latest-news/iso-9001-fdis-update/ ; A https://www.dqsglobal.com/intl/learn/blog/revision-iso-9001 |
| 取適法 (revised 下請法) | in force 2026-01-01; bans one-sided pricing without consultation; scope adds employee-count test | P (via) https://www.kaiketsu-j.com/?p=17632 ; A https://www.yuhikaku.co.jp/static_files/15101_web2.pdf |
| EN 10204 | 3.1 vs 3.2 certificate type must match PO; 3.1 not acceptable where 3.2 specified | A https://blog.projectmaterials.com/epc-projects/testing-inspection/mill-test-certificates-3-1-2 |

---

## 3. Gaps (what no one does well for small/mid manufacturers)

All "nobody does" statements are UNVERIFIED absence findings (see U9).

| # | Gap | Evidence |
|---|---|---|
| G1 | **Cross-document consistency.** Tools digitise one document class each: drawings (CADDi, AiZumen, Werk24), mill sheets (MILL.NET, Mill-Box), inspection forms (Kaminashi, Anlab), cert-vs-grade check (Clappia). Not found: one check across PO requirement ↔ drawing revision/material/tolerance ↔ supplier cert/inspection report, with source locations, tied to the release decision. | §1.2, §1.3 |
| G2 | **Detecting falsification patterns in supplier-submitted data.** Post-scandal guidance focuses on automating capture inside the maker ("人を介さない"); not found: SME tools that screen incoming certificates for statistical/forensic red flags (reused values, impossible precision, post-dating). | §2.1, MONOist 2025 |
| G3 | **Price/fit for SMEs.** Enterprise QMS/PLM is quote-only; SME tools are cheap but single-purpose. Per-supplier fees (1factory $20/supplier/mo) scale against long supplier tails. | §1.1 |
| G4 | **Supplier burden and re-requests.** Per-customer formats, repeated declaration requests after rule changes, low response rates; no tool found that tells a supplier exactly the minimum that must be re-submitted after a change. | §2.3 |
| G5 | **Japanese documents and FAX as evidence.** General AI-OCR reads FAX, but not found: tools keeping FAX transmission header, 和暦 dates, 全角 digits, 印影 as provenance-bearing claims with confidence that gates decisions. | §1.2, §2.2 |
| G6 | **Cross-company proof without data sharing, at SME weight.** Catena-X/Ouranos need connectors and are early; Circularise uses ZK on Ethereum. Not found: a light offline-verifiable receipt/commitment scheme a 20-person shop can run. | §1.4 |
| G7 | **Change impact at the evidence level.** PLM gives BOM "where-used"; not found in SME tools: which past acceptance decisions lose their evidential basis when a drawing, spec, regulation list or certificate changes. | §1.1, R3 |
| G8 | **India/Japan bridge.** Japanese makers in India face BIS/QCO paperwork (71.9% affected per JETRO); Indian exporters must supply verified data for CBAM. Not found: bilingual (JA/EN) evidence checking aimed at this corridor. | §2.4, §2.5 |

---

## 4. Proposed original innovations (11)

Built on existing KIZUNA parts: bitemporal claims with source spans (M2), extraction worker incl. OCR/DXF (M3), entity resolution + 9 conflict kinds `mfg-rules@1` (M3), decisions + typed dependencies + selective invalidation (M4), hash-chained audit + Ed25519 checkpoints (M1/M5), opt-in free/local AI label mapping (M7). Constraints: Rust core, Python extraction, React, Postgres, free/local AI, no paid services.
"Differentiated" = not found in the products in §1 (UNVERIFIED absence).

### I1. Quality three-way match (PO ↔ drawing ↔ certificate)
- **Problem:** CoCs with missing/wrong revision, 3.1 sent where 3.2 ordered, grade/tolerance mismatches are checked by eye (§2.3, EN 10204 row).
- **How:** a `match_set` per PO line: requirement claims from the PO (grade, cert type, qty, drawing no.+rev), design claims from the drawing (material, rev, critical dims/tolerances), evidence claims from mill sheet / 検査成績書. New rule pack `mfg-rules@2` evaluates the triangle; each finding cites three source spans. Result is the input claim set of a `release` decision, so M4 invalidation works unchanged.
- **Different:** AP three-way match is common for invoices; applying it to quality evidence with span-level citations is not seen in §1 tools (which each own one corner).
- **Effort:** M. **Risk:** extraction errors on real layouts (R6). **Measure:** injected-fault recall/precision per mismatch type in `scripts/e2e.sh`; reviewer minutes per PO line (pilot).

### I2. Revision drift radar incl. "silent revisions"
- **Problem:** documents cite stale revisions; drawings sometimes change without a revision bump (hypothesis H, to verify in pilot).
- **How:** content-hash + normalised-text diff of artifacts sharing (drawing no., rev); differing content under the same rev → `silent_revision` conflict. Supplier docs citing rev < current effective rev at their `valid_at` → `stale_reference`. Uses existing content-addressed store and bitemporal `valid_at`.
- **Different:** PLM controls revisions it owns; it does not police external copies circulating by e-mail/FAX.
- **Effort:** S. **Risk:** false positives from re-scans (mitigate: compare extracted claims, not bytes). **Measure:** FP rate on re-scanned identical drawings; recall on injected same-rev edits.

### I3. Certificate forensics signals (not accusations)
- **Problem:** falsification cases (§2.1) involved altered or reused test values; SME receivers have no screen.
- **How:** Rust detector over numeric claims per supplier/part history: identical value vectors across different heats/lots; precision finer than declared gauge resolution; clustering just inside limits; terminal-digit preference (chi-square, only when n is large enough); issue date after ship date (post-dated CoC); same template hash with different mill name. Output = `evidence_signal` with statistic + threshold + data used, routed to human review; never auto-reject. Thresholds per tenant like ADR 0015.
- **Different:** §1 tools check value vs limit; none found screening for manipulation patterns across a supplier's history.
- **Effort:** M. **Risk:** false accusations damaging supplier relations (wording, review-only, audit of who saw it); small samples. **Measure:** synthetic injected falsification patterns (recall) vs honest synthetic noise (FP per 1,000 certs); later blind pilot review.

### I4. Invalidation-driven minimal evidence request (supplier side)
- **Problem:** suppliers get repeated broad re-requests (§2.3).
- **How:** when M4 flags decisions, compute the minimal set of claims whose replacement would restore them (hitting set over the flagged dependency edges); render a bilingual request (JA business format / EN) listing only those items, with the exact clause/drawing rev that changed. Supplier replies (upload or FAX) feed back as claims and auto-reconfirm when rules pass.
- **Different:** turns selective invalidation into a supplier-burden reducer; not seen in supplier portals.
- **Effort:** M. **Risk:** hitting set may be large on dense graphs (bounded greedy + human edit). **Measure:** items requested vs a naive "resubmit everything" baseline on synthetic graphs; supplier turnaround in pilot.

### I5. Japanese-document provenance pack (FAX header, 和暦, 全角, 印影)
- **Problem:** FAX and stamped paper remain (§2.2); OCR output loses where/when it came from.
- **How:** extraction worker adds: FAX transmission header line parse (sender no., time) as a capture-provenance claim; 和暦→ISO date with era table; 全角/半角 and 〜/～ normalisation; 印影 region detection (local OpenCV/heuristics) producing an `approval_mark_present` claim, never "approved" by itself; per-claim OCR confidence + crop. Rule: a decision-critical claim below a confidence floor needs human confirmation before it can support a decision.
- **Different:** generic AI-OCR digitises; this makes the scan's own metadata and confidence part of the evidence model.
- **Effort:** M. **Risk:** 印影 detection accuracy; FAX header formats vary. **Measure:** field accuracy on a labelled synthetic FAX/scan set; % critical claims gated; zero decisions resting on unconfirmed low-confidence claims (property test).

### I6. Regulation-list versions as dependency nodes
- **Problem:** declarations go stale when SVHC/RoHS/QCO lists change, causing blanket re-surveys (§2.3, §2.4).
- **How:** import a versioned regulatory list (e.g. SVHC candidate list snapshot) as an artifact; declaration claims record which list version they answered; a new version adds substances → only declarations for parts whose materials could contain the new substances are flagged (materiality rule).
- **Different:** extends evidence-level invalidation to regulation changes instead of calendar re-surveys.
- **Effort:** M. **Risk:** mapping substances to materials needs data (supplier FMDs); licence of list sources. **Measure:** flagged-declaration count vs blanket re-survey on synthetic data.

### I7. Sealed delivery receipts (cross-company, offline, no chain)
- **Problem:** disputes over "which document did you send / when" and proof without sharing data (G6).
- **How:** on intake, KIZUNA issues the supplier a receipt: artifact SHA-256, received-at, Merkle inclusion proof in the Ed25519-checkpointed audit chain, plus salted commitments to selected claim values. Small offline verifier (Rust CLI, WASM in browser) checks it. Salt policy from PLAN §R (low-entropy leakage).
- **Different:** simpler than ZK/blockchain (Circularise) and data spaces (Catena-X); runs on one server.
- **Effort:** S–M. **Risk:** commitments leak if salts mishandled; legal weight unknown. **Measure:** verifier passes on untouched receipts, fails on every single-bit tamper (property test); verify time.

### I8. Change "what-if" preview before approving an ECN
- **Problem:** impact of a drawing/spec change on past acceptance decisions is unknown until after the change.
- **How:** dry-run of the dependency engine on a hypothetical superseding claim; returns decisions that would be flagged, suppliers to contact (I4), and open shipments affected. No writes; audited as a query.
- **Different:** evidence-level, not BOM where-used.
- **Effort:** S. **Risk:** low. **Measure:** dry-run result ≡ actual result after commit (property test).

### I9. Cross-standard grade guard (JIS ↔ IS ↔ ASTM/EN)
- **Problem:** Japan/India corridor mixes material standards (G8); "equivalent" grades are not identical.
- **How:** grade-equivalence table as a tenant-curated artifact (not shipped content, avoiding standards licensing); mismatched standard → `substitution_requires_decision` conflict; approval becomes a decision with its own evidence.
- **Different:** makes substitutions explicit decisions with evidence, rather than silent lookup.
- **Effort:** M. **Risk:** table quality depends on customer; standards text is copyrighted. **Measure:** injected cross-standard substitutions caught; tenant curation time.

### I10. Evidence debt ledger per part/supplier
- **Problem:** QA leaders cannot see which accepted parts rest on weak/expiring/unconfirmed evidence.
- **How:** score per decision = missing required claims + low-confidence claims + signals (I3) + days to nearest expiry; aggregated per supplier; trend over transaction time (bitemporal). Read-only view.
- **Different:** a liability view of evidence quality, not a task list.
- **Effort:** S. **Risk:** score gaming / false precision (show components, not one number). **Measure:** pilot: does ranking match QA lead's own list (rank correlation).

### I11. Customer-format 検査成績書 renderer from verified claims (supplier mode)
- **Problem:** suppliers re-type values into each customer's template (§2.2).
- **How:** supplier-tenant maps its measured claims once; templates per customer (XLSX/PDF) rendered from claims with a hash footer linking to I7 receipt.
- **Different:** output is backed by the same verifiable claims, removing transcription as an error and fraud channel.
- **Effort:** M. **Risk:** template variety; scope creep into report-writer market. **Measure:** transcription errors (zero by construction, test) and time per report in pilot.

---

## 5. Top 5 for immediate build on this codebase

| Rank | Innovation | Why now | Effort |
|---|---|---|---|
| 1 | I1 Quality three-way match | Direct extension of `mfg-rules@1` + decisions; most visible value in a demo; covers G1 | M |
| 2 | I3 Certificate forensics signals | Strongest Japan-specific story (§2.1); pure Rust engine, synthetic tests possible now | M |
| 3 | I2 Revision drift / silent revision | Small, uses existing content store + bitemporal claims | S |
| 4 | I5 Japanese provenance pack | Needed for any real Japanese pilot (FAX/scan); Python worker only | M |
| 5 | I4 Minimal evidence request | Turns M4 into supplier-burden reduction (G4); pairs with I8 (S) almost free | M |

Next after these: I8 (S, reuse I4 code), I7 (S–M), I6.

---

## 6. Proposed research-register rows (to append to `research-register.md` after review)
| ID | Hypothesis | Experiment | Result |
|---|---|---|---|
| R7 | Three-way match catches revision/cert-type/grade/tolerance mismatches with span citations | injected faults per type in e2e | not run |
| R8 | Forensic signals detect injected reuse/precision/post-dating patterns at < X FP per 1,000 honest certs (X set before run) | synthetic honest vs manipulated generator | not run |
| R9 | Same-rev content diff on extracted claims has near-zero FP on re-scans | re-scan pairs + injected edits | not run |
| R10 | Minimal request set is much smaller than resubmit-all | proptest graphs | not run |
| R11 | Confidence gating keeps every decision free of unconfirmed low-confidence critical claims | property test | not run |
| R12 | Dry-run invalidation equals committed invalidation | property test | not run |

## 7. Proposed unverified-claims rows (to append to `unverified-claims.md`)
| # | Claim | Status |
|---|---|---|
| U9 | "No competitor does G1–G8" | UNVERIFIED absence from public pages; sales calls/demos needed before external use |
| U10 | Silent revisions (same rev, changed content) occur in real supplier flows | HYPOTHESIS; check in pilot data |
| U11 | Kobe Steel 688 customers; SME chemSHERPA cost ¥2.23M/yr | secondary sources; open primary reports before citing |
| U12 | Japan/India market size, willingness to pay | UNKNOWN |

## 8. Not covered / next research
- ESPR DPP delegated-act timeline; Assent/Source Intelligence pricing; ETQ/Intelex/Opcenter prices; Japanese QMS SaaS beyond those listed.
- Practitioner voice (Reddit r/QualityControl, Japanese Q&A sites, X): agent-reach channels not run in this pass; search returned no Reddit threads.
- India: ICRIER 2025 survey, BIS/DPIIT primary texts.
