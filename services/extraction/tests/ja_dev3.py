"""Dev documents for the v3 layout readers (synthetic, with ground truth; split "dev", used for tuning).

Written for ja-v3 from the layout families named in docs/testing/JA_EXTRACTION_V2_2026-11-05.md, without
reading the independent holdout (tests/ja_holdout.py). Families:

- `htable`: horizontal measurement tables (items as columns; rows 規格値 / 公差 / 測定値 / 判定), with ruling
  lines or without, measured rows as one vertically merged label cell or numbered rows.
- `above`: labels above their values in boxed strips, single label/value stacks and value-above-label boxes.
- `multipart`: one document describing two or three parts in sections; truth carries the part (`subject`).
- `sandwich`: a page image with a scanner's hidden text layer (render mode 3) in which some values are near
  misses of the printed value (one character wrong). Truth is the printed value; the review gate must flag
  the wrong ones. Needs ImageMagick (skipped without it).

Distractors: telephone numbers, 規格範囲 ranges, AQL levels and chamfers (C0.5).
"""

from __future__ import annotations

import random

from ja_corpus import (
    ITEMS,
    MATERIALS,
    NAMES,
    PAGE_H,
    PART_NAMES,
    STANDARDS,
    SUPPLIERS,
    TOL,
    D,
    Doc,
    Item,
    N,
    T,
    _date,
    _iso,
    cjk_pdf,
    date_text,
    magick_available,
    render_scan,
    tol_text,
    wide,
)

Rule = tuple[float, float, float, float]

LAB = {  # dev label pools: the train vocabulary plus unknown-but-cued variants
    "part_number": ["品番", "部品番号", "製品番号", "Part No."],
    "part_name": ["品名", "部品名", "製品名"],
    "drawing_number": ["図番", "図面番号", "図面No."],
    "revision": ["改訂", "Rev.", "版数"],
    "material": ["材質", "材料"],
    "quantity": ["数量", "員数"],
    "inspection_date": ["検査日", "検査年月日"],
    "issue_date": ["発行日", "作成日"],
    "supplier": ["仕入先", "製造者"],
    "result": ["総合判定", "判定"],
    "standard": ["規格"],
    "surface_treatment": ["表面処理"],
}
EN_ROWS = {"item": "Item", "nominal": "Nominal (mm)", "tol": "Tol.", "result": "Judge"}


def lab(rnd: random.Random, f: str) -> str:
    return rnd.choice(LAB[f])


def _meas(rnd: random.Random, nom: str, pl: str, mi: str) -> str:
    lo, hi = float(nom) - float(mi), float(nom) + float(pl)
    dec = len(nom.split(".")[1]) if "." in nom else 0
    v = rnd.uniform(lo + (hi - lo) * 0.2, hi - (hi - lo) * 0.2)
    return f"{v:.{dec + 1 if dec < 2 else dec}f}"


def _box(x0: float, y0: float, x1: float, y1: float) -> list[Rule]:
    return [(x0, y0, x1, y0), (x0, y1, x1, y1), (x0, y0, x0, y1), (x1, y0, x1, y1)]


def _grid(xs: list[float], ys: list[float]) -> list[Rule]:
    """Full grid: horizontal lines at ys, vertical lines at xs."""
    return [(xs[0], y, xs[-1], y) for y in ys] + [(x, ys[-1], x, ys[0]) for x in xs]


def _header_kv(rnd: random.Random, fields: list[tuple[str, str, dict]], y: float) -> tuple[list[Item], float]:
    out: list[Item] = []
    for f, v, _t in fields:
        out.append((60, y, 10, f"{lab(rnd, f)}{rnd.choice(['：', ': '])}{v}"))
        y -= 16
    return out, y


# ---------------------------------------------------------------- family 1: horizontal measurement tables
def htable(rnd: random.Random, idx: int) -> tuple[list[list[Item]], list[dict], list[list[Rule]], str]:
    pn = f"HT-{rnd.randint(100, 999)}-{rnd.choice('PQR')}"
    pname = rnd.choice(PART_NAMES)
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    sup = rnd.choice(SUPPLIERS)
    truth = [
        T("part_number", pn),
        T("part_name", pname),
        D("inspection_date", _iso(y, m, d), dt),
        T("supplier", sup),
    ]
    page: list[Item] = [(220, 800, 16, rnd.choice(["検 査 成 績 書", "出荷検査記録", "測定記録表"]))]
    head, yy = _header_kv(
        rnd, [("part_number", pn, {}), ("part_name", pname, {}), ("inspection_date", dt, {})], 760
    )
    page += head
    page.append((330, 760, 9, f"{lab(rnd, 'supplier')}：{sup}"))
    tel = f"TEL 0{rnd.randint(3, 9)}-{rnd.randint(1000, 9999)}-{rnd.randint(1000, 9999)}"
    page.append((330, 746, 9, tel))
    names = rnd.sample(sorted(ITEMS), k=3 + idx % 3)
    english = idx % 4 == 3
    n_meas = 2 + idx % 2
    merged = idx % 3 != 2  # measured rows: one vertically merged label cell with sub numbers
    ruled = idx % 2 == 0
    right_align = ruled and idx % 4 == 0
    unit_row = idx % 5 == 4  # the unit in a separate 単位 row instead of the row label
    x0, sub_x, first = 50.0, 112.0, 150.0
    step = min(80.0 if not right_align else 44.0, (560 - first) / len(names))
    xs = [first + i * step for i in range(len(names))]
    rows: list[tuple[list[str], list[str]]] = []  # (label cells, value cells)
    rows.append(([EN_ROWS["item"] if english else rnd.choice(["検査項目", "項目", "測定項目"])], names))
    nom_lab = EN_ROWS["nominal"] if english else rnd.choice(["規格値 (mm)", "基準寸法(mm)", "規格値(㎜)"])
    if unit_row:
        nom_lab = "規格値"
        rows.append((["単位"], ["mm"] * len(names)))
    noms, tols, meas, oks = [], [], [[] for _ in range(n_meas)], []
    for nm in names:
        suf, nom, (pl, mi), unit = ITEMS[nm]
        noms.append(nom)
        t = tol_text(pl, mi, False)
        tols.append(t)
        truth.append(N(f"dimension.{suf}", nom, unit, nom))
        truth.append(TOL(pl, mi, t))
        for k in range(n_meas):
            mv = _meas(rnd, nom, pl, mi)
            meas[k].append(mv)
            truth.append(N(f"measured.{suf}", mv, unit, mv))
        ok = rnd.choice(["合格", "OK", "○"])
        oks.append(ok)
        truth.append(T("result", "pass", ok))
    rows.append(([nom_lab], noms))
    rows.append(([EN_ROWS["tol"] if english else rnd.choice(["公差", "許容差"])], tols))
    for k in range(n_meas):
        if english:
            rows.append(([f"X{k + 1}"], meas[k]))
        elif merged:
            rows.append(((["測定値"] if k == 0 else []) + [str(k + 1)], meas[k]))
        else:
            rows.append(([rnd.choice(["測定値", "実測値"]) + str(k + 1)], meas[k]))
    rows.append(([EN_ROWS["result"] if english else "判定"], oks))
    yy -= 30
    top = yy + 12
    for labs, vals in rows:
        if len(labs) == 2 or (merged and labs and labs[0].isdigit()):
            if len(labs) == 2:
                page.append((x0 + 4, yy, 9, labs[0]))
            page.append((sub_x + 4, yy, 9, labs[-1]))
        elif labs:
            page.append((x0 + 4, yy, 9, labs[0]))
        for x, v in zip(xs, vals, strict=True):
            if right_align and v not in names:  # values flush right in their ruled cell: tight gaps
                w = sum(9 if wide(ch) else 4.5 for ch in v)
                page.append((x + step - 2 - w, yy, 9, v))
            else:
                page.append((x + 4, yy, 9, v))
        yy -= 16
    rules = _grid([x0, *xs, xs[-1] + step], [top - 16 * i for i in range(len(rows) + 1)]) if ruled else []
    if idx % 2:
        rng = f"{noms[0]}～{float(noms[0]) + 0.05:.2f}"
        page.append((60, yy - 20, 9, f"規格範囲 {rng}  AQL 1.0  面取り C0.5"))
    res_lab = lab(rnd, "result")
    page.append((60, yy - 40, 10, f"{res_lab}：合格"))
    truth.append(T("result", "pass", "合格"))
    return [page], truth, [rules], "inspection_report"


# ---------------------------------------------------------------- family 2: labels above values
def above(rnd: random.Random, idx: int) -> tuple[list[list[Item]], list[dict], list[list[Rule]], str]:
    dn = f"DW-{rnd.randint(10000, 99999)}"
    pn = f"AB-{rnd.randint(1000, 9999)}"
    pname = rnd.choice(PART_NAMES)
    mat = rnd.choice(MATERIALS)
    rev = rnd.choice("ABCD")
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    drawer, approver = rnd.sample(NAMES, 2)
    fields = [
        ("part_number", pn, T("part_number", pn)),
        ("part_name", pname, T("part_name", pname)),
        ("drawing_number", dn, T("drawing_number", dn)),
        ("material", mat, T("material", mat)),
        ("revision", rev, T("revision", rev)),
        ("issue_date", dt, D("issue_date", _iso(y, m, d), dt)),
    ]
    rnd.shuffle(fields)
    truth = [t for *_x, t in fields]
    page: list[Item] = [(200, 800, 16, rnd.choice(["部品仕様書", "図面", "製品規格書"]))]
    rules: list[Rule] = []
    style = idx % 3
    if style == 0:  # boxed strips: a row of labels, the values in the row below
        y0 = 740.0
        for chunk in (fields[:3], fields[3:]):
            w = 160.0
            for i, (f, v, _t) in enumerate(chunk):
                bx = 50 + i * w
                page.append((bx + 6 + rnd.uniform(0, 30), y0, 8, lab(rnd, f)))
                page.append((bx + 6, y0 - 18, 10, v))
                rules += _box(bx, y0 - 24, bx + w, y0 + 12)
                rules.append((bx, y0 - 6, bx + w, y0 - 6))
            y0 -= 60
    elif style == 1:  # vertical stacks: one label line, one value line, in two columns
        for i, (f, v, _t) in enumerate(fields):
            cx = 60 if i % 2 == 0 else 320
            ry = 740 - (i // 2) * 44
            page.append((cx, ry, 8, lab(rnd, f)))
            page.append((cx + rnd.choice([0, 8]), ry - 16, 11, v))
    else:  # value above its label (title block boxes with the caption at the bottom)
        for i, (f, v, _t) in enumerate(fields):
            bx = 50 + (i % 3) * 170
            by = 700 - (i // 3) * 50
            page.append((bx + 8, by, 11, v))
            page.append((bx + 8, by - 16, 7, lab(rnd, f)))
            rules += _box(bx, by - 22, bx + 170, by + 16)
    # approval boxes: role above the name
    page.append((380, 600, 8, "作図"))
    page.append((440, 600, 8, "承認"))
    page.append((380, 584, 10, drawer))
    page.append((440, 584, 10, approver))
    truth += [T("drawn_by", drawer), T("approved_by", approver)]
    for r, lb in (("drawn", "作図"), ("approved", "承認")):
        truth.append({"field": f"stamp.{r}", "text": "present", "raw": lb})
    page.append((60, 560, 9, f"TEL 06-{rnd.randint(1000, 9999)}-{rnd.randint(1000, 9999)}"))
    if idx % 2:
        std = STANDARDS[mat]
        page.append((60, 540, 9, f"{lab(rnd, 'standard')}：{std}"))
        truth.append(T("standard", std))
    return [page], truth, [rules], "drawing" if idx % 2 == 0 else "specification"


# ---------------------------------------------------------------- family 3: several parts in one document
def multipart(rnd: random.Random, idx: int) -> tuple[list[list[Item]], list[dict], list[list[Rule]], str]:
    n_parts = 2 + idx % 2
    sup = rnd.choice(SUPPLIERS)
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    page: list[Item] = [(200, 800, 16, rnd.choice(["部品仕様一覧", "検査成績書（複数品番）"]))]
    page.append((60, 770, 10, f"{lab(rnd, 'supplier')}：{sup}"))
    page.append((60, 754, 10, f"{lab(rnd, 'issue_date')}：{dt}"))
    truth: list[dict] = []
    yy = 720.0
    parts = []
    for k in range(n_parts):
        pn = f"MP-{rnd.randint(1000, 9999)}-{k + 1}"
        pname = PART_NAMES[(idx + k) % len(PART_NAMES)]
        mat = rnd.choice(MATERIALS)
        parts.append(pn)
        hdr = rnd.choice([f"【部品{k + 1}】", f"■ No.{k + 1}", f"{k + 1}."])
        page.append((50, yy, 11, hdr))
        yy -= 18
        sub = [
            T("part_number", pn),
            T("part_name", pname),
            T("material", mat),
            T("supplier", sup),
            D("issue_date", _iso(y, m, d), dt),
        ]
        for f, v in (("part_number", pn), ("part_name", pname), ("material", mat)):
            page.append((70, yy, 10, f"{lab(rnd, f)}：{v}"))
            yy -= 15
        # a short dimension list per part
        for nm in rnd.sample(sorted(ITEMS), 2):
            suf, nom, (pl, mi), unit = ITEMS[nm]
            t = tol_text(pl, mi, False)
            page.append((70, yy, 10, f"{nm}：{nom}mm {t}" if (idx + k) % 2 else f"{nm}(mm)：{nom} {t}"))
            sub.append(N(f"dimension.{suf}", nom, unit, nom))
            sub.append(TOL(pl, mi, t))
            yy -= 15
        truth += [{**t, "subject": pn} for t in sub]
        yy -= 14
    return [page], truth, [[]], "specification"


# ---------------------------------------------------------------- family 3b: traps for the geometric readers
def nearmiss(rnd: random.Random, idx: int) -> tuple[list[list[Item]], list[dict], list[list[Rule]], str]:
    """Traps a positional reader can fall into: a second (customer / old) part number beside the real one,
    an empty box in a label strip, footer numbers, an unruled horizontal table with a missing measurement,
    centred values under wide item names and a 規格範囲 (range) row."""
    pn = f"NM-{rnd.randint(1000, 9999)}"
    other = f"{rnd.choice(['C', 'X'])}{rnd.randint(100000, 999999)}"
    pname = rnd.choice(PART_NAMES)
    trt = rnd.choice(["三価クロメート", "黒染め", "アルマイト"])
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    page: list[Item] = [(200, 800, 16, rnd.choice(["検査成績書", "納入仕様書"]))]
    page.append((60, 770, 10, f"{lab(rnd, 'part_number')}：{pn}"))
    page.append((300, 770, 10, f"{rnd.choice(['客先品番', '旧品番', '顧客品番'])}：{other}"))
    page.append((60, 754, 10, f"{lab(rnd, 'inspection_date')}：{dt}"))
    truth = [T("part_number", pn), D("inspection_date", _iso(y, m, d), dt)]
    # label strip with an empty box (材質 not filled in)
    labs = [("part_name", pname), ("material", ""), ("surface_treatment", trt)]
    y0, w = 720.0, 150.0
    rules: list[Rule] = []
    for i, (f, v) in enumerate(labs):
        bx = 50 + i * w
        caption = lab(rnd, f) if f != "surface_treatment" else "表面処理"
        page.append((bx + 6 + rnd.uniform(0, 40), y0, 8, caption))
        if v:
            page.append((bx + 6 + rnd.uniform(0, 20), y0 - 18, 10, v))
        rules += _box(bx, y0 - 24, bx + w, y0 + 12)
    truth += [T("part_name", pname), T("surface_treatment", trt)]
    # unruled horizontal table: centred values, one measurement missing, a range row
    names = rnd.sample(sorted(ITEMS), 3)
    xs = [170.0 + i * 120 for i in range(3)]
    yy = 660.0

    def put(label: str, vals: list[str]) -> None:
        nonlocal yy
        page.append((54, yy, 9, label))
        for x, v in zip(xs, vals, strict=True):
            if v:
                w2 = sum(9 if wide(ch) else 4.5 for ch in v)
                page.append((x + 30 - w2 / 2, yy, 9, v))
        yy -= 16

    put("検査項目", [f"{nm}（図面指示）" if idx % 2 else nm for nm in names])
    noms, tols, ms = [], [], []
    for nm in names:
        suf, nom, (pl, mi), unit = ITEMS[nm]
        noms.append(nom)
        tols.append(tol_text(pl, mi, False))
        truth += [N(f"dimension.{suf}", nom, unit, nom), TOL(pl, mi, tols[-1])]
        mv = _meas(rnd, nom, pl, mi)
        ms.append(mv)
    put("規格値(mm)", noms)
    put("公差", tols)
    put("規格範囲", [f"{float(n) - 0.05:.2f}～{float(n) + 0.05:.2f}" for n in noms])
    missing = idx % 3
    ms[missing] = ""
    put("測定値", ms)
    for nm, mv in zip(names, ms, strict=True):
        if mv:
            suf, *_rest = ITEMS[nm]
            truth.append(N(f"measured.{suf}", mv, "mm", mv))
    tel = f"TEL 052-{rnd.randint(100, 999)}-{rnd.randint(1000, 9999)}"
    fax = f"FAX 052-{rnd.randint(100, 999)}-{rnd.randint(1000, 9999)}"
    page.append((60, yy - 30, 8, f"{tel}  {fax}"))
    page.append((280, 40, 8, "Page 1/1"))
    return [page], truth, [rules], "inspection_report"


# ---------------------------------------------------------------- family 4: hidden layers with near misses
NEAR = {"0": "8", "8": "3", "3": "8", "1": "7", "7": "1", "5": "6", "6": "5", "2": "7", "9": "4", "4": "9"}


def near_miss(rnd: random.Random, v: str) -> str:
    pos = [i for i, c in enumerate(v) if c in NEAR]
    if not pos:
        return v
    i = rnd.choice(pos)
    return v[:i] + NEAR[v[i]] + v[i + 1 :]


def sandwich(rnd: random.Random, idx: int) -> tuple[list[Item], list[Item], list[dict], str]:
    """(printed page, hidden text layer, truth, kind): the layer is the printed page with near misses."""
    pages, truth, _rules, kind = htable(rnd, idx) if idx % 2 == 0 else above(rnd, idx)
    printed = pages[0]
    hidden: list[Item] = []
    wrong = 0
    for x, y, s, t in printed:
        if any(c.isdigit() for c in t) and rnd.random() < 0.18:
            t2 = near_miss(rnd, t)
            wrong += t2 != t
            hidden.append((x, y, s, t2))
        else:
            hidden.append((x, y, s, t))
    if wrong == 0:  # every sandwich document carries at least one wrong value
        i = next(i for i, (*_x, t) in enumerate(hidden) if any(c.isdigit() for c in t))
        x, y, s, t = hidden[i]
        hidden[i] = (x, y, s, near_miss(rnd, t))
    return printed, hidden, truth, kind


# ---------------------------------------------------------------- corpus
def build_dev3(seed: int = 20261106, n: int = 6, scans: bool = True) -> list[Doc]:
    docs: list[Doc] = []
    for name, fn in (("htable", htable), ("above", above), ("multipart", multipart), ("nearmiss", nearmiss)):
        for i in range(n):
            rnd = random.Random(f"{seed}:dev3:{name}:{i}")
            pages, truth, rules, kind = fn(rnd, i)
            per_char = i % 3 == 1
            docs.append(
                Doc(
                    f"dev/{name}-{i:02d}",
                    kind,
                    "application/pdf",
                    cjk_pdf(pages, per_char, rules=rules),
                    truth,
                    "dev",
                    "text-per-char" if per_char else "text",
                    pages,
                )
            )
    if scans and magick_available():
        for i in range(4):
            rnd = random.Random(f"{seed}:dev3:sandwich:{i}")
            printed, hidden, truth, kind = sandwich(rnd, i)
            jpg = render_scan(printed, [], "jpeg", seed + i)
            docs.append(
                Doc(
                    f"dev/sandwich-{i:02d}",
                    kind,
                    "application/pdf",
                    cjk_pdf([hidden], False, images=[jpg], invisible=True),
                    truth,
                    "dev",
                    "sandwich",
                    [printed],
                )
            )
    return docs


__all__ = ["build_dev3", "PAGE_H"]
