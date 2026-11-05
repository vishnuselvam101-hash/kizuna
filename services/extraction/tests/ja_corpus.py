"""Deterministic labelled corpus of Japanese manufacturing documents (synthetic, with ground truth).

Every document is built from positioned text items, so the same layout can be written as a text PDF (CJK Type0
font with a ToUnicode map, optionally one fragment per character like Excel/Word exports), as a Shift_JIS CSV,
or rendered to a scanned PNG/PDF with ImageMagick for the OCR path (noise, slight skew, red 印影 stamps).

Ground truth entries are canonical: {"field", "text"} | {"field", "number", "unit"} | {"field", "date"} |
{"field", "plus", "minus"} plus "raw" (the printed text). `split` is "train" (labels the rules were written
against) or "holdout" (synonyms and layouts deliberately NOT added to the rules, to measure generalisation).
"""

from __future__ import annotations

import random
import shutil
import subprocess
import unicodedata
from dataclasses import dataclass, field

Item = tuple[float, float, float, str]  # x, y (PDF points, origin bottom-left), font size, text

FONT = "Noto-Sans-CJK-JP"
PAGE_W, PAGE_H = 595.0, 842.0


@dataclass
class Doc:
    name: str
    kind: str  # artifact kind used by the Rust mapping
    media_type: str
    data: bytes
    truth: list[dict]
    split: str = "train"
    variant: str = "text"
    pages: list[list[Item]] = field(default_factory=list)
    stamps: list[tuple[int, float, float, str]] = field(default_factory=list)  # page, x, y, name


# ---------------------------------------------------------------- text helpers
FW = str.maketrans({chr(c): chr(c + 0xFEE0) for c in range(0x21, 0x7F)})


def fullwidth(s: str) -> str:
    return s.translate(FW)


def wide(ch: str) -> bool:
    return unicodedata.east_asian_width(ch) in ("W", "F")


ERAS = (("令和", 2018), ("平成", 1988))


def wareki(y: int, m: int, d: int, style: int) -> str:
    era, base = ERAS[0] if (y, m, d) >= (2019, 5, 1) else ERAS[1]
    ey = y - base
    eys = "元" if ey == 1 and style == 0 else str(ey)
    if style == 0:
        return f"{era}{eys}年{m}月{d}日"
    if style == 1:
        return f"{'R' if era == '令和' else 'H'}{ey}.{m}.{d}"
    return f"{y}年{m}月{d}日"


def date_text(rnd: random.Random, y: int, m: int, d: int) -> str:
    s = rnd.randrange(4)
    return f"{y}/{m:02d}/{d:02d}" if s == 3 else wareki(y, m, d, s)


def num_text(rnd: random.Random, s: str, fw_rate: float) -> str:
    return fullwidth(s) if rnd.random() < fw_rate else s


def T(f: str, text: str, raw: str | None = None) -> dict:  # noqa: N802
    return {"field": f, "text": text, "raw": raw if raw is not None else text}


def N(f: str, number: str, unit: str | None, raw: str) -> dict:  # noqa: N802
    return {"field": f, "number": number, "unit": unit or "", "raw": raw}


def D(f: str, iso: str, raw: str) -> dict:  # noqa: N802
    return {"field": f, "date": iso, "raw": raw}


def TOL(plus: str, minus: str, raw: str) -> dict:  # noqa: N802
    return {"field": "tolerance", "plus": plus, "minus": minus, "raw": raw}


# ---------------------------------------------------------------- vocabularies
LABELS_TRAIN = {
    "part_number": ["品番", "部品番号", "品目コード"],
    "part_name": ["品名", "部品名"],
    "drawing_number": ["図番", "図面番号"],
    "revision": ["改訂", "Rev.", "版数"],
    "material": ["材質", "材料"],
    "lot": ["ロットNo.", "ロット番号"],
    "quantity": ["数量", "員数"],
    "inspection_date": ["検査日", "検査年月日"],
    "supplier": ["仕入先", "製造者", "発行者"],
    "result": ["総合判定", "判定"],
    "heat_number": ["溶鋼番号", "チャージNo.", "ヒートNo."],
    "standard": ["規格"],
    "issue_date": ["発行日", "作成日"],
    "scale": ["尺度"],
    "surface_treatment": ["表面処理"],
    "po_number": ["注文番号", "発注番号", "注文No."],
    "delivery_date": ["納期", "納入日"],
    "unit_price": ["単価"],
}
LABELS_HOLDOUT = {
    "part_number": ["製品番号", "部品No."],
    "part_name": ["製品名"],
    "drawing_number": ["図面No."],
    "revision": ["改訂記号"],
    "material": ["使用材料"],
    "lot": ["製造ロット"],
    "quantity": ["個数"],
    "inspection_date": ["検査実施日"],
    "supplier": ["納入業者"],
    "result": ["合否"],
    "heat_number": ["鋼番"],
    "standard": ["適用規格"],
    "issue_date": ["発行年月日"],
    "scale": ["スケール"],
    "surface_treatment": ["表面仕上げ"],
    "po_number": ["発注No."],
    "delivery_date": ["納入期日"],
    "unit_price": ["単価(円)"],
}
ITEMS = {  # inspection item → (field suffix, nominal, tolerance (plus, minus), unit)
    "外径": ("outer_diameter", "25.00", ("0", "0.05"), "mm"),
    "内径": ("inner_diameter", "12.00", ("0.05", "0"), "mm"),
    "全長": ("length", "120.5", ("0.1", "0.1"), "mm"),
    "幅": ("width", "40.0", ("0.2", "0.2"), "mm"),
    "厚さ": ("thickness", "3.20", ("0.05", "0.05"), "mm"),
    "高さ": ("height", "18.0", ("0.1", "0.1"), "mm"),
}
ITEMS_HOLDOUT = {
    "板厚": ("thickness", "6.00", ("0.1", "0.1"), "mm"),
    "長さ": ("length", "250.0", ("0.5", "0.5"), "mm"),
}
MATERIALS = ["SS400", "S45C", "SUS304", "A5052P", "SCM435", "SPCC"]
STANDARDS = {
    "SS400": "JIS G 3101",
    "S45C": "JIS G 4051",
    "SUS304": "JIS G 4304",
    "SCM435": "JIS G 4053",
    "SPCC": "JIS G 3141",
    "A5052P": "JIS H 4000",
}
NAMES = ["山田", "佐藤", "鈴木", "高橋", "田中", "伊藤", "渡辺", "中村"]
SUPPLIERS = ["株式会社山本製作所", "東和精工株式会社", "有限会社大西金属", "北陸製鋼株式会社"]
PART_NAMES = ["ブラケット", "シャフト", "フランジ", "スペーサー", "カバー", "ブッシュ"]
TREATMENTS = ["三価クロメート", "無電解ニッケルめっき", "黒染め", "アルマイト"]


def labels(rnd: random.Random, split: str, f: str) -> str:
    pool = LABELS_HOLDOUT if split == "holdout" and f in LABELS_HOLDOUT else LABELS_TRAIN
    return rnd.choice(pool[f])


def tol_text(plus: str, minus: str, fw: bool) -> str:
    s = f"±{plus}" if plus == minus else f"+{plus}/−{minus}"
    return fullwidth(s).replace("＋", "＋").replace("／", "／") if fw else s


def kv(rnd: random.Random, label: str, value: str) -> str:
    sep = rnd.choice(["：", ": ", "："])
    return f"{label}{sep}{value}"


# ---------------------------------------------------------------- document builders (positioned items)
def _date(rnd: random.Random) -> tuple[int, int, int]:
    return rnd.choice([2019, 2024, 2025]), rnd.randint(1, 12), rnd.randint(1, 28)


def _iso(y: int, m: int, d: int) -> str:
    return f"{y:04d}-{m:02d}-{d:02d}"


def inspection_report(rnd: random.Random, split: str, idx: int) -> tuple[list[list[Item]], list[dict], list]:
    fwr = 0.3 if idx % 2 else 0.0
    pn = f"KZ-{rnd.randint(1000, 9999)}-{rnd.choice('ABC')}"
    dn = f"DW-{rnd.randint(10000, 99999)}"
    rev = rnd.choice("ABCD")
    lot = f"L{rnd.randint(2401, 2512)}-{rnd.randint(1, 30):02d}"
    qty = rnd.choice([100, 500, 1200, 2500])
    qty_s = f"{qty:,}"
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    sup = rnd.choice(SUPPLIERS)
    pname = rnd.choice(PART_NAMES)
    truth = [
        T("part_number", pn),
        T("part_name", pname),
        T("drawing_number", dn),
        T("revision", rev),
        T("lot", lot),
        N("quantity", str(qty), "pcs", f"{qty_s} 個"),
        D("inspection_date", _iso(y, m, d), dt),
        T("supplier", sup),
        T("result", "pass", "合格"),
    ]
    p1: list[Item] = [(220, 800, 16, "検 査 成 績 書")]
    yy = 760
    for f, v in (
        ("part_number", pn),
        ("part_name", pname),
        ("drawing_number", dn),
        ("revision", rev),
        ("lot", lot),
        ("quantity", num_text(rnd, qty_s, fwr) + " 個"),
        ("inspection_date", num_text(rnd, dt, fwr)),
        ("supplier", sup),
    ):
        p1.append((60, yy, 10, kv(rnd, labels(rnd, split, f), v)))
        yy -= 16
    items = dict(ITEMS)
    if split == "holdout":
        items.update(ITEMS_HOLDOUT)
    names = rnd.sample(sorted(items), k=min(len(items), 5 + idx % 3))
    unit_hdr = rnd.choice(["規格値 (mm)", "規格値(㎜)", "基準寸法 (mm)"])
    meas_hdr = rnd.choice([["測定値1", "測定値2", "測定値3"], ["X1", "X2", "X3"], ["n=1", "n=2", "n=3"]])
    cols = [60, 140, 225, 320, 380, 440, 500]
    header = ["検査項目", unit_hdr, "公差", *meas_hdr, "判定"]
    rows: list[list[str]] = []
    for nm in names:
        suf, nom, (pl, mi), unit = items[nm]
        meas = []
        for _ in range(3):
            lo, hi = float(nom) - float(mi), float(nom) + float(pl)
            dec = len(nom.split(".")[1]) if "." in nom else 0
            v = rnd.uniform(lo + (hi - lo) * 0.2, hi - (hi - lo) * 0.2)
            meas.append(f"{v:.{dec + 1 if dec < 2 else dec}f}")
        ok = rnd.choice(["合格", "○", "OK"])
        tol = tol_text(pl, mi, rnd.random() < fwr)
        rows.append([nm, num_text(rnd, nom, fwr), tol, *meas, ok])
        truth.append(N(f"dimension.{suf}", nom, unit, nom))
        truth.append(TOL(pl, mi, tol))
        for mv in meas:
            truth.append(N(f"measured.{suf}", mv, unit, mv))
        truth.append(T("result", "pass", ok))
    if idx % 3 == 0:  # hardness row with the unit in the item name
        hv = str(rnd.randint(50, 58))
        rows.append(["硬さ (HRC)", "", "", hv, "", "", "合格"])
        truth.append(N("property.hardness", hv, "hrc", hv))
        truth.append(T("result", "pass", "合格"))
    pages = [p1]
    page, yy = p1, yy - 20
    repeat_header = idx % 2 == 0
    split_at = 4 if idx % 4 == 1 else 99  # multi-page table: rows continue on page 2

    def put_row(pg: list[Item], y: float, cells: list[str]) -> None:
        for x, c in zip(cols, cells, strict=False):
            if c:
                pg.append((x, y, 9, c))

    put_row(page, yy, header)
    for i, r in enumerate(rows):
        if i == split_at:
            page = []
            pages.append(page)
            yy = 780
            if repeat_header:
                put_row(page, yy, header)
        yy -= 14
        put_row(page, yy, r)
    page.append((60, yy - 40, 10, kv(rnd, labels(rnd, split, "result"), "合格")))
    return pages, truth, []


def mill_cert(rnd: random.Random, split: str, idx: int) -> tuple[list[list[Item]], list[dict], list]:
    fwr = 0.5 if idx % 2 else 0.0
    mat = rnd.choice(["SS400", "S45C", "SCM435", "SPCC"])
    std = STANDARDS[mat]
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    sup = rnd.choice(SUPPLIERS[3:] + SUPPLIERS[:1])
    mpa_unit = rnd.choice(["N/mm²", "MPa", "N/mm2"])
    truth = [
        T("material", mat),
        T("standard", std),
        D("issue_date", _iso(y, m, d), dt),
        T("supplier", sup),
        T("part_name", "熱間圧延鋼板"),
    ]
    p: list[Item] = [(200, 800, 16, "ミルシート（材料証明書）"), (200, 782, 9, "INSPECTION CERTIFICATE")]
    yy = 750
    for lab, val in (
        (labels(rnd, split, "issue_date"), num_text(rnd, dt, fwr)),
        (labels(rnd, split, "supplier"), sup),
        (labels(rnd, split, "standard"), std),
        ("鋼種", mat),
        ("品名", "熱間圧延鋼板"),
    ):
        p.append((60, yy, 10, kv(rnd, lab, val)))
        yy -= 16
    yy -= 14
    elems = ["C", "Si", "Mn", "P", "S"]
    hx = [60, 118, 170, 222, 274, 326, 380, 470, 555]
    p.append((118, yy, 8, "化学成分 (%)"))
    p.append((380, yy, 8, "機械的性質"))
    yy -= 14
    heat_lab = labels(rnd, split, "heat_number")
    sub = [heat_lab, *elems, f"降伏点 ({mpa_unit})", f"引張強さ ({mpa_unit})", "伸び (%)"]
    for x, c in zip(hx, sub, strict=True):
        p.append((x, yy, 8, c))
    for _ in range(1 + idx % 3):
        yy -= 14
        heat = f"{rnd.choice('ABKM')}{rnd.randint(10000, 99999)}"
        chem = [
            f"0.{rnd.randint(12, 45)}",
            f"0.{rnd.randint(15, 35)}",
            f"{rnd.randint(30, 140) / 100:.2f}",
            f"0.0{rnd.randint(10, 30)}",
            f"0.00{rnd.randint(3, 9)}",
        ]
        ys, ts, el = str(rnd.randint(245, 400)), str(rnd.randint(400, 620)), str(rnd.randint(18, 32))
        cells = [heat, *chem, ys, ts, el]
        for x, c in zip(hx, cells, strict=True):
            p.append((x, yy, 8, num_text(rnd, c, fwr)))
        truth.append(T("heat_number", heat))
        for e, v in zip(elems, chem, strict=True):
            truth.append(N(f"property.chem_{e.lower()}", v, "%", v))
        truth += [
            N("property.yield_strength", ys, "mpa", ys),
            N("property.tensile_strength", ts, "mpa", ts),
            N("property.elongation", el, "%", el),
        ]
    p.append((60, yy - 40, 10, kv(rnd, "判定", "合格")))
    truth.append(T("result", "pass", "合格"))
    return [p], truth, []


def spec_doc(rnd: random.Random, split: str, idx: int) -> tuple[list[list[Item]], list[dict], list]:
    fwr = 0.4 if idx % 2 else 0.0
    pn = f"KZ-{rnd.randint(1000, 9999)}"
    mat = rnd.choice(MATERIALS)
    hr = str(rnd.randint(40, 58))
    ra = rnd.choice(["0.8", "1.6", "3.2"])
    trt = rnd.choice(TREATMENTS)
    rev = rnd.choice("ABC")
    y, m, d = _date(rnd)
    dt = date_text(rnd, y, m, d)
    ln, od = "120.5", "25"
    lt = rnd.choice([("0.1", "0.1"), ("0.2", "0")])
    ot = ("0", "0.05")
    lt_s, ot_s = tol_text(*lt, rnd.random() < fwr), tol_text(*ot, rnd.random() < fwr)
    leader = idx % 3 == 2  # "材質 ……… S45C" style
    entries = [
        (labels(rnd, split, "part_number"), pn, [T("part_number", pn)]),
        (labels(rnd, split, "part_name"), "シャフト", [T("part_name", "シャフト")]),
        (labels(rnd, split, "material"), mat, [T("material", mat)]),
        ("硬さ", f"HRC{num_text(rnd, hr, fwr)}", [N("property.hardness", hr, "hrc", f"HRC{hr}")]),
        ("表面粗さ", f"Ra {ra}", [N("property.surface_roughness", ra, "ra", f"Ra {ra}")]),
        (labels(rnd, split, "surface_treatment"), trt, [T("surface_treatment", trt)]),
        (
            "全長",
            f"{num_text(rnd, ln, fwr)}{rnd.choice(['㎜', ' mm'])} {lt_s}",
            [N("dimension.length", ln, "mm", ln), TOL(*lt, lt_s)],
        ),
        ("外径", f"φ{od} {ot_s}", [N("dimension.outer_diameter", od, "", od), TOL(*ot, ot_s)]),
        (labels(rnd, split, "revision"), rev, [T("revision", rev)]),
        (labels(rnd, split, "issue_date"), num_text(rnd, dt, fwr), [D("issue_date", _iso(y, m, d), dt)]),
        ("承認", "済", [T("approval_status", "approved", "済")]),
    ]
    p: list[Item] = [(240, 800, 16, "製 品 仕 様 書")]
    yy = 760
    truth: list[dict] = []
    for i, (lab, val, tr) in enumerate(entries, start=1):
        text = f"{i}. {lab} ……… {val}" if leader else kv(rnd, lab, val)
        p.append((60, yy, 10, text))
        yy -= 18
        truth += tr
    return [p], truth, []


def drawing(rnd: random.Random, split: str, idx: int) -> tuple[list[list[Item]], list[dict], list]:
    fwr = 0.3 if idx % 2 else 0.0
    dn = f"DW-{rnd.randint(10000, 99999)}"
    pname = rnd.choice(PART_NAMES)
    mat = rnd.choice(MATERIALS)
    scale = rnd.choice(["1:1", "1:2", "2:1", "1:5"])
    rev = rnd.choice("ABCDE")
    trt = rnd.choice(TREATMENTS)
    who = rnd.sample(NAMES, 3)
    y, m, d = _date(rnd)
    dts = [wareki(y, m, d, 1), wareki(y, m, d, 1), wareki(y, m, d, 1)]
    truth = [
        T("drawing_number", dn),
        T("part_name", pname),
        T("material", mat),
        T("scale", scale),
        T("revision", rev),
        T("surface_treatment", trt),
        T("drawn_by", who[0]),
        T("checked_by", who[1]),
        T("approved_by", who[2]),
        D("approval_date", _iso(y, m, d), dts[2]),
    ]
    p: list[Item] = [(60, 800, 12, "注記"), (60, 784, 9, "1. 指示なき寸法公差は JIS B 0405-m による。")]
    # dimension table (item / nominal / tolerance)
    yy = 740
    for x, c in zip([60, 140, 230], ["項目", "寸法 (mm)", "公差"], strict=True):
        p.append((x, yy, 9, c))
    for nm in rnd.sample(["外径", "全長", "厚さ", "幅"], 3):
        suf, nom, (pl, mi), unit = ITEMS[nm]
        tol = tol_text(pl, mi, rnd.random() < fwr)
        yy -= 14
        for x, c in zip([60, 140, 230], [nm, num_text(rnd, nom, fwr), tol], strict=True):
            p.append((x, yy, 9, c))
        truth += [N(f"dimension.{suf}", nom, unit, nom), TOL(pl, mi, tol)]
    # title block: label/value pairs on one row (a key-value grid, not a table)
    lab = {f: labels(rnd, split, f) for f in ("drawing_number", "part_name", "material", "scale", "revision")}
    tb = [
        [lab["drawing_number"], dn, lab["part_name"], pname],
        [lab["material"], mat, lab["scale"], scale],
        [lab["revision"], rev, labels(rnd, split, "surface_treatment"), trt],
    ]
    xs = [330, 380, 460, 510]
    yy = 160
    for row in tb:
        for x, c in zip(xs, row, strict=True):
            p.append((x, yy, 9, c))
        yy -= 14
    # approval stamps: role row, name row (stamps), date row
    yy -= 10
    roles = ["設計", "検図", "承認"]
    sx = [340, 400, 460]
    for x, c in zip(sx, roles, strict=True):
        p.append((x, yy, 9, c))
    stamps = []
    for x, nm in zip(sx, who, strict=True):  # stamp box: about 30 pt high, 印影 about 22 pt across
        p.append((x, yy - 30, 9, nm))
        stamps.append((1, x + 9, yy - 27, nm))
    for x, dtx in zip(sx, dts, strict=True):
        p.append((x - 4, yy - 54, 8, dtx))
    for r in roles:
        truth.append(
            {
                "field": f"stamp.{ {'設計': 'drawn', '検図': 'checked', '承認': 'approved'}[r] }",
                "text": "present",
                "raw": r,
            }
        )
    return [p], truth, stamps


def purchase_order(rnd: random.Random, split: str, idx: int) -> tuple[list[list[str]], list[dict]]:
    po = f"PO-{rnd.randint(250000, 259999)}"
    sup = rnd.choice(SUPPLIERS)
    y, m, d = _date(rnd)
    rows: list[list[str]] = []
    truth: list[dict] = []
    if idx % 2 == 0:  # preamble before the header row
        rows += [["注文書"], [labels(rnd, split, "issue_date"), date_text(rnd, y, m, d)], []]
        truth.append(D("issue_date", _iso(y, m, d), rows[1][1]))
    hdr_f = [
        "po_number",
        "po_line",
        "part_number",
        "part_name",
        "quantity",
        "unit_price",
        "delivery_date",
        "supplier",
    ]
    header = [labels(rnd, split, f) if f != "po_line" else "行" for f in hdr_f]
    header.insert(5, "単位")
    rows.append(header)
    for line in range(1, 2 + idx % 4 + 1):
        pn = f"KZ-{rnd.randint(1000, 9999)}"
        q = rnd.choice([50, 200, 1000, 12000])
        price = rnd.choice([85, 1250, 3400, 15800])
        yy, mm, dd = _date(rnd)
        dd_s = date_text(rnd, yy, mm, dd)
        price_s = rnd.choice([f"￥{price:,}", f"{price:,}円", f"{price:,}"])
        rows.append([po, str(line), pn, rnd.choice(PART_NAMES), f"{q:,}", "個", price_s, dd_s, sup])
        truth += [
            T("po_number", po),
            T("po_line", str(line)),
            T("part_number", pn),
            T("part_name", rows[-1][3]),
            N("quantity", str(q), "pcs", f"{q:,}"),
            N("unit_price", str(price), "jpy" if price_s[-1] == "円" or price_s[0] == "￥" else "", price_s),
            D("delivery_date", _iso(yy, mm, dd), dd_s),
            T("supplier", sup),
        ]
    return rows, truth


# ---------------------------------------------------------------- writers
def _hex(s: str, cid: dict[str, int]) -> str:
    return "".join(f"{cid[c]:04X}" for c in s)


def cjk_pdf(pages: list[list[Item]], per_char: bool = False) -> bytes:
    """Text PDF with an unembedded CJK Type0 font (Identity-H) and a ToUnicode CMap, like many JP exports."""
    chars = sorted({c for p in pages for *_, t in p for c in t})
    cid = {c: i + 1 for i, c in enumerate(chars)}
    contents = []
    for p in pages:
        ops = []
        for x, y, size, t in p:
            if per_char:
                cx = x
                for c in t:
                    if c != " ":
                        ops.append(f"BT /F1 {size} Tf {cx:.2f} {y:.2f} Td <{_hex(c, cid)}> Tj ET")
                    cx += size * (1.0 if wide(c) else 0.5)
            else:
                ops.append(f"BT /F1 {size} Tf {x:.2f} {y:.2f} Td <{_hex(t, cid)}> Tj ET")
        contents.append("\n".join(ops).encode())
    n = len(pages)
    objs: list[bytes] = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        f"<< /Type /Pages /Kids [{' '.join(f'{3 + 2 * i} 0 R' for i in range(n))}] /Count {n} >>".encode(),
    ]
    font = 3 + 2 * n
    for i, body in enumerate(contents):
        objs.append(
            f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {PAGE_W:.0f} {PAGE_H:.0f}] "
            f"/Contents {4 + 2 * i} 0 R "
            f"/Resources << /Font << /F1 {font} 0 R >> >> >>".encode()
        )
        objs.append(f"<< /Length {len(body)} >>\nstream\n".encode() + body + b"\nendstream")
    half = " ".join(f"{cid[c]} [500]" for c in chars if not wide(c))
    objs.append(
        f"<< /Type /Font /Subtype /Type0 /BaseFont /NotoSansCJKjp-Regular /Encoding /Identity-H "
        f"/DescendantFonts [{font + 1} 0 R] /ToUnicode {font + 3} 0 R >>".encode()
    )
    objs.append(
        f"<< /Type /Font /Subtype /CIDFontType2 /BaseFont /NotoSansCJKjp-Regular /CIDSystemInfo << /Registry "
        f"(Adobe) /Ordering (Identity) /Supplement 0 >> /FontDescriptor {font + 2} 0 R /DW 1000 "
        f"/W [{half}] >>".encode()
    )
    objs.append(
        b"<< /Type /FontDescriptor /FontName /NotoSansCJKjp-Regular /Flags 4 /FontBBox [0 -120 1000 880] "
        b"/ItalicAngle 0 /Ascent 880 /Descent -120 /CapHeight 700 /StemV 80 >>"
    )
    cmap = [
        "/CIDInit /ProcSet findresource begin 12 dict begin begincmap",
        "/CIDSystemInfo << /Registry (Adobe) /Ordering (UCS) /Supplement 0 >> def",
        "/CMapName /Adobe-Identity-UCS def /CMapType 2 def",
        "1 begincodespacerange <0000> <FFFF> endcodespacerange",
    ]
    items = list(cid.items())
    for k in range(0, len(items), 100):
        chunk = items[k : k + 100]
        cmap.append(f"{len(chunk)} beginbfchar")
        cmap += [f"<{v:04X}> <{ord(c):04X}>" for c, v in chunk]
        cmap.append("endbfchar")
    cmap.append("endcmap CMapName currentdict /CMap defineresource pop end end")
    cm = "\n".join(cmap).encode()
    objs.append(f"<< /Length {len(cm)} >>\nstream\n".encode() + cm + b"\nendstream")
    out = b"%PDF-1.4\n"
    offs = []
    for i, o in enumerate(objs, start=1):
        offs.append(len(out))
        out += f"{i} 0 obj\n".encode() + o + b"\nendobj\n"
    x = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offs)
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    return out


def magick_available() -> bool:
    return shutil.which("magick") is not None


def render_scan(
    page: list[Item], stamps: list[tuple[float, float, str]], fmt: str, seed: int, header: str | None = None
) -> bytes:
    """Renders one page at 200 dpi with ImageMagick: noise, slight skew, red circular 印影 for stamps."""
    k = 200 / 72
    argv = [
        "magick",
        "-size",
        f"{int(PAGE_W * k)}x{int(PAGE_H * k)}",
        "xc:white",
        "-font",
        FONT,
        "-fill",
        "black",
    ]

    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("%", "%%")

    if header:
        argv += ["-pointsize", str(int(8 * k)), "-annotate", f"+{int(20 * k)}+{int(14 * k)}", esc(header)]
    names = {nm for *_xy, nm in stamps}
    for x, y, size, t in page:
        if t in names:
            continue  # the name is inside the red stamp instead
        argv += [
            "-pointsize",
            str(int(size * k)),
            "-annotate",
            f"+{int(x * k)}+{int((PAGE_H - y) * k)}",
            esc(t),
        ]
    for x, y, nm in stamps:
        cx, cy, r = int(x * k), int((PAGE_H - y) * k), int(11 * k)
        argv += ["-fill", "none", "-stroke", "#d02020", "-strokewidth", "4"]
        argv += ["-draw", f"circle {cx},{cy} {cx + r},{cy}", "-stroke", "none", "-fill", "#d02020"]
        argv += ["-pointsize", str(int(8 * k)), "-annotate", f"+{cx - int(8 * k)}+{cy + int(3 * k)}", esc(nm)]
        argv += ["-fill", "black"]
    rnd = random.Random(seed)
    argv += ["-seed", str(seed), "-attenuate", "0.25", "+noise", "Gaussian", "-background", "white"]
    argv += [
        "-rotate",
        f"{rnd.uniform(-0.6, 0.6):.2f}",
        "-alpha",
        "off",
        "-colorspace",
        "sRGB",
        "-depth",
        "8",
        "-strip",
    ]
    argv += ["-units", "PixelsPerInch", "-density", "200"]
    argv += ["-compress", "JPEG", "-quality", "85"] if fmt == "pdf" else []
    argv += [f"{fmt}:-"]
    return subprocess.run(argv, capture_output=True, check=True, timeout=120).stdout  # noqa: S603


def sjis_csv(rows: list[list[str]]) -> bytes:
    return (
        "\r\n".join(",".join(f'"{c}"' if "," in c else c for c in r) for r in rows).encode("cp932") + b"\r\n"
    )


# ---------------------------------------------------------------- corpus
BUILDERS = {
    "inspection_report": inspection_report,
    "mill_cert": mill_cert,
    "spec": spec_doc,
    "drawing": drawing,
}
KIND = {
    "inspection_report": "inspection_report",
    "mill_cert": "certificate",
    "spec": "specification",
    "drawing": "drawing",
}


def build(seed: int = 20261105, per_type: int = 6, scans: bool = True) -> list[Doc]:
    docs: list[Doc] = []
    for split, n in (("train", per_type), ("holdout", max(2, per_type // 3))):
        for t, fn in BUILDERS.items():
            for i in range(n):
                rnd = random.Random(f"{seed}:{split}:{t}:{i}")
                pages, truth, stamps = fn(rnd, split, i)
                per_char = i % 3 == 1
                docs.append(
                    Doc(
                        f"{split}/{t}-{i:02d}",
                        KIND[t],
                        "application/pdf",
                        cjk_pdf(pages, per_char),
                        truth,
                        split,
                        "text-per-char" if per_char else "text",
                        pages,
                        stamps,
                    )
                )
        for i in range(n):
            rnd = random.Random(f"{seed}:{split}:po:{i}")
            rows, truth = purchase_order(rnd, split, i)
            docs.append(
                Doc(
                    f"{split}/po-csv-{i:02d}",
                    "purchase_order",
                    "text/csv",
                    sjis_csv(rows),
                    truth,
                    split,
                    "csv",
                )
            )
    if scans and magick_available():
        docs += scan_docs(seed)
    return docs


FAX_FROM = "株式会社山本製作所"


def scan_docs(seed: int) -> list[Doc]:
    out: list[Doc] = []
    for t in ("inspection_report", "drawing", "spec", "mill_cert"):
        for i in range(2):
            rnd = random.Random(f"{seed}:scan:{t}:{i}")
            pages, truth, stamps = BUILDERS[t](rnd, "train", i * 2)
            pages = pages[:1]
            header = None
            if i == 1:  # FAX-received copy: transmission header line on top
                y, m, d = 2025, rnd.randint(1, 12), rnd.randint(1, 28)
                hh, mi = rnd.randint(8, 18), rnd.randint(0, 59)
                header = (
                    f"{y}/{m:02d}/{d:02d} {hh:02d}:{mi:02d}  FROM:{FAX_FROM}  FAX:03-5432-1098  P.001/001"
                )
                truth = [
                    *truth,
                    T("fax.sender", FAX_FROM),
                    T("fax.number", "03-5432-1098"),
                    {
                        "field": "fax.sent_at",
                        "date": f"{y}-{m:02d}-{d:02d}T{hh:02d}:{mi:02d}",
                        "raw": header[:16],
                    },
                ]
            fmt = "png" if i == 0 else "pdf"
            st = [(x, y, nm) for _p, x, y, nm in stamps]
            data = render_scan(pages[0], st, fmt, seed + i, header)
            mt = "image/png" if fmt == "png" else "application/pdf"
            out.append(
                Doc(f"scan/{t}-{i:02d}", KIND[t], mt, data, truth, "scan", f"scan-{fmt}", pages, stamps)
            )
    return out
