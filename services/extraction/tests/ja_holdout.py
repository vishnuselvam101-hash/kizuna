"""Independent holdout corpus of Japanese manufacturing paperwork (synthetic, exact ground truth).

Written without looking at the extraction rules or the training corpus builders: own label wording,
layouts, date/tolerance spellings, ID formats, CSV dialects and scan renderer. Only the shared Doc/truth
helpers and the text-PDF writer of ja_corpus are reused.

Conventions: numeric "raw" is the printed number only (the unit may sit in a header or after the value);
tolerance "plus"/"minus" are signed decimals as printed ("±0.05" -> "0.05"/"-0.05", "+0.03/0" -> "0.03"/"0").
"""

from __future__ import annotations

import csv
import io
import random
import shutil
import subprocess
import sys
import unicodedata
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from ja_corpus import PAGE_H, PAGE_W, TOL, D, Doc, Item, N, T, cjk_pdf, fullwidth  # noqa: E402

SUPPLIERS = [
    "株式会社三河精工",
    "有限会社浜松プレス工業",
    "東邦メタル株式会社",
    "㈱北陸ファスナー",
    "(株)岡谷ギヤテック",
    "株式会社 豊橋熱処理",
    "新居浜鍛造株式会社",
    "有限会社 刈谷精密",
]
MILLS = ["北越鋼業株式会社", "瀬戸内スチール株式会社", "東海特殊鋼株式会社", "日向製鋼所"]
BUYERS = ["キズナ工業株式会社", "中央総合機工株式会社", "株式会社サクラ電装"]
PARTS = [
    "シャフト",
    "フランジ",
    "スリーブ",
    "ブッシュ",
    "スパーギヤ",
    "ノックピン",
    "カラー",
    "ブラケット",
    "ハウジング",
    "スペーサ",
    "軸受ホルダ",
    "ローラ軸",
    "ジョイントピン",
    "プーリ",
]
PEOPLE = ["佐藤", "鈴木", "高橋", "田中", "伊藤", "渡辺", "山本", "中村", "小林", "加藤", "吉田", "松本"]
TREATMENTS = [
    "無電解Niめっき",
    "三価クロメート",
    "黒染め",
    "白アルマイト",
    "リン酸塩皮膜処理",
    "硬質クロムめっき",
    "亜鉛めっき(三価)",
    "パーカー処理",
]
MATS = {  # grade: (standard, ranges for C Si Mn P S; P/S as max)
    "S45C": ("JIS G 4051", (0.42, 0.48), (0.15, 0.35), (0.60, 0.90), 0.030, 0.035),
    "S50C": ("JIS G 4051", (0.47, 0.53), (0.15, 0.35), (0.60, 0.90), 0.030, 0.035),
    "SCM440": ("JIS G 4053", (0.38, 0.43), (0.15, 0.35), (0.60, 0.90), 0.030, 0.030),
    "SCr420": ("JIS G 4053", (0.18, 0.23), (0.15, 0.35), (0.60, 0.90), 0.030, 0.030),
    "SUJ2": ("JIS G 4805", (0.95, 1.10), (0.15, 0.35), (0.10, 0.50), 0.025, 0.025),
    "SUS304": ("JIS G 4303", (0.03, 0.08), (0.30, 1.00), (0.80, 2.00), 0.045, 0.030),
    "SS400": ("JIS G 3101", (0.12, 0.20), (0.10, 0.30), (0.40, 0.80), 0.050, 0.050),
}
PART_MATS = ["S45C", "SCM440", "SUS304", "SS400", "A5052", "SUJ2", "C3604BD", "S50C", "SCr420", "SPCC"]
TELS = ["0566-21-4410", "053-468-2207", "076-251-9934", "0532-45-1180", "0897-33-6021", "052-871-3305"]


def nk(s: str) -> str:
    return unicodedata.normalize("NFKC", s)


def fw(rnd: random.Random, s: str, rate: float = 0.2) -> str:
    return fullwidth(s) if rnd.random() < rate else s


def jdate(rnd: random.Random, y: int, m: int, d: int) -> str:
    r = y - 2018
    return rnd.choice(
        [
            f"{y}.{m}.{d}",
            f"R{r}/{m}/{d}",
            f"令和{fullwidth(str(r))}年{fullwidth(str(m))}月{fullwidth(str(d))}日",
            f"{y}年{m:02d}月{d:02d}日",
            f"{y}-{m:02d}-{d:02d}",
            f"{y}/{m}/{d}",
            f"R{r:02d}.{m:02d}.{d:02d}",
            f"令和{r}年{m}月{d}日",
            f"{y}年{m}月{d}日",
        ]
    )


def rdate(rnd: random.Random) -> tuple[int, int, int]:
    return rnd.choice([2024, 2025, 2026]), rnd.randint(1, 12), rnd.randint(1, 28)


def iso(ymd: tuple[int, int, int]) -> str:
    return f"{ymd[0]:04d}-{ymd[1]:02d}-{ymd[2]:02d}"


def tol(rnd: random.Random) -> tuple[str, str, str]:
    """Returns (raw, plus, minus)."""
    a = rnd.choice(["0.1", "0.05", "0.02", "0.2", "0.03"])
    b = rnd.choice(["0.01", "0.02", "0.05", "0.1"])
    k = rnd.randrange(6)
    if k == 0:
        return f"±{a}", a, f"-{a}"
    if k == 1:
        return f"+{a}/-{b}", a, f"-{b}"
    if k == 2:
        return f"＋{a}／－{b}", a, f"-{b}"
    if k == 3:
        return f"+{a}/0", a, "0"
    if k == 4:
        return f"0/-{b}", "0", f"-{b}"
    return f"±{fullwidth(a)}", a, f"-{a}"


def part_no(rnd: random.Random) -> str:
    k = rnd.randrange(5)
    if k == 0:
        return f"{rnd.choice('ABCDM')}{rnd.randint(10, 99)}-{rnd.randint(1000, 9999)}-{rnd.randint(1, 9):02d}"
    if k == 1:
        return f"{rnd.randint(2, 9)}{rnd.choice('GHKT')}-{rnd.randint(100, 999)}{rnd.choice('ABC')}"
    if k == 2:
        return f"MS-{rnd.randint(2000, 2999)}-{rnd.randint(1, 30):02d}"
    if k == 3:
        return f"{rnd.randint(50000, 59999)}-{rnd.choice(['K27', 'T05', 'R1A'])}-{rnd.choice('ABX')}"
    return f"HB{rnd.randint(1000, 9999)}{rnd.choice('RLN')}"


def dwg_no(rnd: random.Random) -> str:
    return rnd.choice(
        [
            f"DWG-{rnd.randint(1000, 9999)}-{rnd.randint(1, 9)}",
            f"Z-{rnd.randint(20000, 29999)}",
            f"S{rnd.randint(1, 4)}-{rnd.randint(100, 999):04d}{rnd.choice('AB')}",
            f"{rnd.randint(10, 99)}A{rnd.randint(1000, 9999)}",
        ]
    )


def lot_no(rnd: random.Random) -> str:
    return rnd.choice(
        [
            f"L{rnd.randint(240101, 261228)}-{rnd.randint(1, 9):02d}",
            f"{rnd.randint(24, 26)}{rnd.randint(1, 12):02d}{rnd.choice('ABC')}{rnd.randint(10, 99)}",
            f"{rnd.randint(1, 9)}C-{rnd.randint(100, 999)}",
        ]
    )


class Pg:
    def __init__(self) -> None:
        self.items: list[Item] = []

    def t(self, x: float, y: float, size: float, text: str) -> None:
        self.items.append((round(x, 1), round(y, 1), size, text))


def kv(p: Pg, rnd: random.Random, pairs: list[tuple[str, str]], x0: float, y0: float, style: int) -> float:
    """Lays out label/value pairs; returns the y below the block."""
    y = y0
    if style == 0:  # "ラベル：値" in one fragment, two columns
        for i, (lab, val) in enumerate(pairs):
            p.t(x0 + (i % 2) * 260, y, 10, f"{lab}{rnd.choice(['：', ':', ' ', '　'])}{val}")
            if i % 2:
                y -= 17
        return y - (17 if len(pairs) % 2 else 0) - 6
    if style == 1:  # label then value 95pt to the right
        for i, (lab, val) in enumerate(pairs):
            x = x0 + (i % 2) * 260
            p.t(x, y, 9, lab)
            p.t(x + 95, y, 10, val)
            if i % 2:
                y -= 17
        return y - (17 if len(pairs) % 2 else 0) - 6
    if style == 2:  # small label above value, three per row
        for i, (lab, val) in enumerate(pairs):
            x = x0 + (i % 3) * 172
            p.t(x, y, 7, lab)
            p.t(x + 4, y - 13, 11, val)
            if i % 3 == 2:
                y -= 34
        return y - (34 if len(pairs) % 3 else 0) - 4
    # grid: label cell | narrow ":" cell | value cell (two cells to the right)
    for lab, val in pairs:
        p.t(x0, y, 9, lab)
        p.t(x0 + 120, y, 9, "｜")
        p.t(x0 + 175, y, 10, val)
        y -= 16
    return y - 6


# ---------------------------------------------------------------- 検査成績書
DIM_LABELS = {
    "outer_diameter": ["外径", "外径 φD", "O.D.", "外 径"],
    "inner_diameter": ["内径", "I.D.", "穴径", "内径 φd"],
    "length": ["全長", "長さ", "L寸法", "Length"],
    "width": ["幅", "巾", "Width"],
    "thickness": ["板厚", "厚さ", "肉厚"],
    "height": ["高さ", "H寸法", "Height"],
}
DIM_RANGE = {
    "outer_diameter": (12, 80, 2),
    "inner_diameter": (6, 40, 2),
    "length": (20, 220, 1),
    "width": (10, 90, 2),
    "thickness": (1.6, 12, 2),
    "height": (8, 60, 1),
}
PASS = ["合格", "OK", "○", "良", "適合", "合 格"]
FAIL = ["不合格", "NG", "×", "否"]


def nominal(rnd: random.Random, dim: str) -> tuple[float, int]:
    lo, hi, dp = DIM_RANGE[dim]
    return round(rnd.uniform(lo, hi), 0 if rnd.random() < 0.3 else 1), dp


def inspection(rnd: random.Random, single: bool = False) -> tuple[list[Pg], list[dict], list]:
    p, tr = Pg(), []
    title = rnd.choice(
        ["検査成績書", "出荷検査成績書", "検 査 表", "受入検査記録", "INSPECTION REPORT 検査成績表"]
    )
    p.t(200, 800, 17, title)
    if rnd.random() < 0.7:
        p.t(470, 818, 8, f"No. {rnd.randint(1000, 9999)}-{rnd.randint(10, 99)}")
    p.t(520, 30, 8, rnd.choice(["1/1", "Page 1 of 1", "- 1 -"]))
    buyer = rnd.choice(BUYERS)
    p.t(40, 770, 11, f"{buyer} 御中")
    sup = rnd.choice(SUPPLIERS)
    sup_in_head = rnd.random() < 0.5
    if sup_in_head:
        p.t(360, 772, 10, sup)
        p.t(360, 759, 8, f"TEL {rnd.choice(TELS)}")
        tr.append(T("supplier", nk(sup), sup))
    pn, pname, dn = part_no(rnd), rnd.choice(PARTS), dwg_no(rnd)
    rev, lot = rnd.choice(["A", "B", "C", "D", "02", "03"]), lot_no(rnd)
    q = rnd.choice([50, 120, 300, 500, 1000, 1200, 2400, 4800])
    qs = f"{q:,}" if q >= 1000 and rnd.random() < 0.6 else str(q)
    qs = fw(rnd, qs)
    qform = rnd.randrange(3)
    ymd = rdate(rnd)
    ds = jdate(rnd, *ymd)
    L = rnd.choice  # noqa: N806
    pairs = [
        (L(["品番", "品番(Part No.)", "P/N", "部品番号", "品目コード"]), pn),
        (L(["品名", "部品名称", "Part Name", "品名(Description)", "名称"]), pname),
        (L(["図番", "図面番号", "Drawing No.", "DWG No."]), dn),
        (L(["改訂", "Rev.", "版数", "改訂記号"]), rev),
        (L(["ロットNo.", "LOT", "製番", "ロット番号", "製造番号"]), lot),
        (
            L(["数量", "出荷数", "納入数量", "Q'ty"]) + ("(個)" if qform == 0 else ""),
            qs + ("" if qform == 0 else L(["個", " pcs", "本", "ケ"])),
        ),
        (L(["検査日", "検査年月日", "Inspection Date", "測定日", "検査実施日"]), ds),
    ]
    tr += [
        T("part_number", pn),
        T("part_name", pname),
        T("drawing_number", dn),
        T("revision", rev),
        T("lot", lot),
        N("quantity", nk(qs).replace(",", ""), "pcs", qs),
        D("inspection_date", iso(ymd), ds),
    ]
    if not sup_in_head:
        pairs.append((L(["納入者", "製造者", "Supplier", "仕入先", "メーカー"]), sup))
        tr.append(T("supplier", nk(sup), sup))
    hard = rnd.random() < 0.6
    if hard:
        hv = f"{rnd.randint(22, 62)}" + (f".{rnd.randint(0, 9)}" if rnd.random() < 0.3 else "")
        hk = rnd.randrange(3)
        if hk == 0:
            pairs.append((L(["硬さ", "表面硬さ", "Hardness"]), f"HRC {hv}"))
        elif hk == 1:
            pairs.append((L(["硬さ", "硬度"]), f"{hv} HRC"))
        else:
            pairs.append((L(["硬度(HRC)", "硬さ (HRC)"]), hv))
        tr.append(N("property.hardness", hv, "hrc", hv))
    pairs.append((L(["検査者", "担当"]), rnd.choice(PEOPLE)))  # distractor
    if rnd.random() < 0.5:
        pairs.append(("抜取数", f"n={rnd.choice([3, 5])}"))  # distractor
    rnd.shuffle(pairs)
    y = kv(p, rnd, pairs, 40, 735, rnd.randrange(4))

    dims = rnd.sample(list(DIM_LABELS), rnd.randint(2, 4))
    n = rnd.randint(3, 5)
    fail = rnd.random() < 0.18
    y -= 10
    p.t(40, y, 10, rnd.choice(["■ 寸法検査", "【測定結果】", "1. 寸法 (Dimensions)"]))
    unit_note = rnd.choice(["(単位：mm)", "単位 ｍｍ", "寸法単位 mm"])
    p.t(430, y, 8, unit_note)
    y -= 20
    show_range = rnd.random() < 0.5
    tol_sep = rnd.random() < 0.5
    ncols = rnd.choice(
        [
            [f"x{i + 1}" for i in range(n)],
            [f"n{i + 1}" for i in range(n)],
            ["①②③④⑤"[i] for i in range(n)],
            [f"測定{i + 1}" for i in range(n)],
        ]
    )
    head = ["No.", rnd.choice(["検査項目", "項目", "特性"]), rnd.choice(["規格値", "図面寸法", "基準寸法"])]
    if tol_sep:
        head.append(rnd.choice(["公差", "許容差"]))
    if show_range:
        head.append(rnd.choice(["規格範囲", "合否範囲"]))
    head += ncols
    head.append(rnd.choice(["測定器", "備考"]))
    widths = {"公差": 56, "許容差": 56, "規格範囲": 76, "合否範囲": 76}
    xs = [40, 62, 120]
    for h in head[2:-1]:  # each column's width decides where the next one starts
        xs.append(xs[-1] + (widths.get(h, 40) if h not in head[2:3] else 52 if tol_sep else 90))
    for x, h in zip(xs, head, strict=True):
        p.t(x, y, 8, h)
    y -= 16
    bad_row = rnd.randrange(len(dims)) if fail else -1
    for r, dim in enumerate(dims):
        nom, dp = nominal(rnd, dim)
        tr_raw, plus, minus = tol(rnd)
        ns = f"{nom:.{dp}f}"
        nsp = ("φ" if "diameter" in dim and rnd.random() < 0.6 else "") + ns
        cells = [str(r + 1), rnd.choice(DIM_LABELS[dim])]
        cells.append(nsp if tol_sep else f"{nsp} {tr_raw}")
        if tol_sep:
            cells.append(tr_raw)
        if show_range:
            lo, hi = nom + float(minus), nom + float(plus)
            cells.append(f"{lo:.{dp}f}～{hi:.{dp}f}")
        tr += [N(f"dimension.{dim}", ns, "mm", ns), TOL(plus, minus, tr_raw)]
        for j in range(n):
            span = float(plus) - float(minus)
            v = nom + float(minus) + span * rnd.uniform(0.15, 0.85)
            if r == bad_row and j == n - 1:
                v = nom + float(plus) + rnd.choice([0.02, 0.04, 0.1])
            vs = f"{v:.{dp + 1 if dp < 3 and rnd.random() < 0.3 else dp}f}"
            cells.append(vs)
            tr.append(N(f"measured.{dim}", vs, "mm", vs))
        cells.append(rnd.choice(["ノギス", "マイクロメータ", "三次元測定機", "ハイトゲージ", "ピンゲージ"]))
        for x, c in zip(xs, cells, strict=True):
            p.t(x, y, 8, c)
        y -= 15
    y -= 14
    if rnd.random() < 0.5:
        p.t(40, y, 8, "外観：キズ・打痕・バリなきこと")
        y -= 14
    res = rnd.choice(FAIL if fail else PASS)
    rl = rnd.choice(["総合判定", "判定", "合否", "Judgement", "検査結果"])
    if rnd.random() < 0.5:
        p.t(330, y, 11, f"{rl}：{res}")
    else:
        p.t(330, y, 9, rl)
        p.t(330, y - 20, 14, res)
    tr.append(T("result", "fail" if fail else "pass", res))
    p.t(
        40,
        60,
        7,
        rnd.choice(["※ 測定室温度 20℃", "注) 本成績書の無断複製を禁ず", "※ 記載なき項目は図面による"]),
    )
    seals = [(470, 120, rnd.choice(PEOPLE), "検査印")]
    p.t(440, 140, 8, "検査印")
    p.t(462, 116, 8, seals[0][2])
    return [p], tr, seals


# ---------------------------------------------------------------- ミルシート
def certificate(rnd: random.Random, single: bool = False) -> tuple[list[Pg], list[dict], list]:
    p, tr = Pg(), []
    grade = rnd.choice(list(MATS))
    std, *rng = MATS[grade]
    p.t(
        150,
        805,
        16,
        rnd.choice(
            [
                "鋼材検査証明書",
                "ミルシート (MILL SHEET)",
                "材料試験成績書",
                "検査証明書 INSPECTION CERTIFICATE",
            ]
        ),
    )
    p.t(40, 780, 10, f"{rnd.choice(BUYERS)} 殿")
    mill = rnd.choice(MILLS)
    p.t(370, 780, 10, mill)
    p.t(370, 767, 8, f"TEL {rnd.choice(TELS)}")
    tr.append(T("supplier", nk(mill), mill))
    p.t(370, 754, 8, f"証明書番号 MC-{rnd.randint(24, 26)}-{rnd.randint(1000, 9999)}")
    heat = rnd.choice(
        [
            f"H{rnd.randint(1, 9)}K{rnd.randint(1000, 9999)}",
            f"{rnd.randint(1, 9)}M{rnd.randint(1000, 9999)}",
            f"S{rnd.randint(24, 26)}-{rnd.randint(10000, 99999)}",
        ]
    )
    pname = rnd.choice(["みがき棒鋼", "熱間圧延丸棒", "冷間引抜棒鋼", "熱間圧延鋼板", "研削棒鋼"])
    ymd = rdate(rnd)
    ds = jdate(rnd, *ymd)
    std_raw = rnd.choice([std, std.replace(" ", ""), std.replace("JIS ", "JIS")])
    L = rnd.choice  # noqa: N806
    pairs = [
        (L(["溶鋼番号", "チャージNo.", "Heat No.", "溶解番号", "鋼番"]), heat),
        (L(["鋼種", "材質", "Grade", "材質記号"]), grade),
        (L(["規格", "適用規格", "Specification"]), std_raw),
        (L(["品名", "製品名", "Commodity"]), pname),
        (L(["発行日", "Date of Issue", "証明日", "発行年月日"]), ds),
        ("寸法", f"φ{rnd.choice([16, 22, 32, 45, 60])}×{rnd.choice([3000, 4000, 5500])}"),  # distractor
        ("重量", f"{rnd.randint(300, 4000):,} kg"),  # distractor
        ("本数", f"{rnd.randint(2, 40)}本"),  # distractor
    ]
    tr += [
        T("heat_number", heat),
        T("material", grade),
        T("standard", nk(std_raw), std_raw),
        T("part_name", pname),
        D("issue_date", iso(ymd), ds),
    ]
    rnd.shuffle(pairs)
    y = kv(p, rnd, pairs, 40, 725, rnd.choice([0, 1, 3]))
    y -= 8
    unit = rnd.choice(["(%)", "(wt%)", "%"])
    p.t(40, y, 10, rnd.choice(["化学成分", "Chemical Composition", "化学成分値"]) + f" {unit}")
    y -= 18
    els = ["C", "Si", "Mn", "P", "S"] + rnd.sample(["Cu", "Ni", "Cr", "Mo"], rnd.randint(0, 3))
    xs = [40 + 105 + 52 * i for i in range(len(els))]
    p.t(40, y, 8, rnd.choice(["区分", "成分値", "成分"]))
    for x, e in zip(xs, els, strict=True):
        p.t(x, y, 9, e)
    y -= 15
    vals = []
    for k in range(len(els)):
        if k < 3:
            lo, hi = rng[k]
            vals.append(f"{rnd.uniform(lo, hi):.2f}")
        elif k < 5:
            vals.append(f"{rnd.uniform(0.004, rng[k] * 0.9):.3f}")
        else:
            vals.append(f"{rnd.uniform(0.01, 0.2):.2f}")
    spec_rows = rnd.random() < 0.6
    if spec_rows:
        mins = [f"{rng[k][0]:.2f}" if k < 3 else "－" for k in range(5)] + ["－"] * (len(els) - 5)
        maxs = [f"{rng[k][1]:.2f}" if k < 3 else f"{rng[k]:.3f}" for k in range(5)] + ["0.30"] * (
            len(els) - 5
        )
        for lab, row in ((L(["規格 MIN", "下限"]), mins), (L(["規格 MAX", "上限"]), maxs)):
            p.t(40, y, 8, lab)
            for x, v in zip(xs, row, strict=True):
                p.t(x, y, 8, v)
            y -= 14
    p.t(40, y, 8, rnd.choice(["分析値", "取鍋分析", "Ladle", "結果"]))
    for k, (x, v) in enumerate(zip(xs, vals, strict=True)):
        pv = fw(rnd, v, 0.1)
        p.t(x, y, 9, pv)
        if k < 5:
            tr.append(N(f"property.chem_{els[k].lower()}", v, "%", pv))
    y -= 28
    p.t(40, y, 10, rnd.choice(["機械的性質", "Mechanical Properties", "引張試験"]))
    y -= 18
    yp = rnd.randint(300, 820)
    ts = yp + rnd.randint(90, 260)
    el = f"{rnd.randint(12, 32)}" + (f".{rnd.randint(0, 9)}" if rnd.random() < 0.4 else "")
    su = rnd.choice(["N/mm²", "MPa"])
    labels = [
        L(["降伏点", "耐力", "YP", "0.2%耐力"]),
        L(["引張強さ", "TS", "Tensile Strength"]),
        L(["伸び", "EL", "Elongation"]),
    ]
    heads = [f"{labels[0]} ({su})", f"{labels[1]} ({su})", f"{labels[2]} (%)"]
    xs = [140, 280, 420]
    for x, h in zip(xs, heads, strict=True):
        p.t(x, y, 8, h)
    y -= 15
    if rnd.random() < 0.5:
        p.t(40, y, 8, "規格値")
        for x, v in zip(
            xs, [f"≧{yp - rnd.randint(40, 120)}", f"≧{ts - rnd.randint(40, 120)}", "≧12"], strict=True
        ):
            p.t(x, y, 8, v)
        y -= 14
    p.t(40, y, 8, rnd.choice(["試験値", "実測", "Result"]))
    for x, v in zip(xs, [str(yp), str(ts), el], strict=True):
        p.t(x, y, 9, v)
    tr += [
        N("property.yield_strength", str(yp), "mpa", str(yp)),
        N("property.tensile_strength", str(ts), "mpa", str(ts)),
        N("property.elongation", el, "%", el),
    ]
    y -= 34
    res = rnd.choice(["合格", "良", "OK", "GOOD"])
    tr.append(T("result", "pass", res))
    p.t(40, y, 10, f"{rnd.choice(['判定', '総合判定', 'Judgement'])}  {res}")
    p.t(40, 70, 7, "上記の通り製造し、規格に適合していることを証明します。")
    p.t(
        40,
        58,
        7,
        "We hereby certify that the material described herein has been made in accordance with the order.",
    )
    p.t(450, 100, 8, "品質保証部")
    return [p], tr, []


# ---------------------------------------------------------------- 仕様書
def specification(rnd: random.Random, single: bool = False) -> tuple[list[Pg], list[dict], list]:
    p, tr = Pg(), []
    p.t(190, 805, 17, rnd.choice(["購入仕様書", "部品仕様書", "製品仕様書", "外注品 仕様書"]))
    p.t(450, 822, 8, f"文書番号 SP-{rnd.randint(100, 999)}-{rnd.randint(1, 9)}")
    pn, pname, mat = part_no(rnd), rnd.choice(PARTS), rnd.choice(PART_MATS)
    rev = rnd.choice(["A", "B", "C", "3", "第2版"])
    rev_val = rev[1] if rev.startswith("第") else rev
    ymd = rdate(rnd)
    ds = jdate(rnd, *ymd)
    app = rnd.choice(["承認済", "承認済み", "APPROVED", "Approved", "承認"])
    L = rnd.choice  # noqa: N806
    pairs = [
        (L(["品番", "P/N", "品番(Part No.)", "部品コード"]), pn),
        (L(["品名", "部品名", "名称"]), pname),
        (L(["改訂", "版", "Rev."]), rev),
        (L(["制定日", "発行日", "改訂日", "Issue Date"]), ds),
        (L(["状態", "承認区分", "Status"]), app),
    ]
    tr += [
        T("part_number", pn),
        T("part_name", pname),
        T("revision", rev_val, rev_val),
        D("issue_date", iso(ymd), ds),
        T("approval_status", "approved", app),
    ]
    rnd.shuffle(pairs)
    y = kv(p, rnd, pairs, 40, 770, rnd.randrange(4))
    hist = rnd.random() < 0.5
    if hist:
        p.t(40, y, 8, "改訂履歴")
        old = rdate(rnd)
        p.t(110, y, 8, f"初版 {old[0] - 1}.{old[1]}.{old[2]} 新規制定")
        y -= 20
    sections: list[tuple[str, list[tuple[str, dict | None]]]] = []
    std = {"A5052": "JIS H 4000", "C3604BD": "JIS H 3250", "SPCC": "JIS G 3141"}.get(mat) or MATS[mat][0]
    sections.append(("適用範囲", [(f"本仕様書は{rnd.choice(BUYERS)}向け{pname}の購入に適用する。", None)]))
    sections.append(("材料", [(f"材質：{mat} ({std}) またはこれと同等以上", None)]))
    tr.append(T("material", mat))
    body = []
    if rnd.random() < 0.8:
        hv = str(rnd.randint(25, 60))
        hs = rnd.choice([f"HRC {hv}以上", f"{hv}HRC 以上", f"HRC{hv}"])
        body.append((f"熱処理後の硬さ：{hs}", N("property.hardness", hv, "hrc", hv)))
    st = rnd.choice(TREATMENTS)
    body.append(
        (f"{rnd.choice(['表面処理', '処理', '仕上げ処理'])}：{st}", T("surface_treatment", nk(st), st))
    )
    ra = rnd.choice(["0.8", "1.6", "3.2", "6.3", "0.4"])
    body.append(
        (
            f"{rnd.choice(['表面粗さ', '仕上げ面粗さ', '粗さ'])}："
            + rnd.choice([f"Ra {ra}", f"Ra{ra}", f"Ra{ra} 以下"]),
            N("property.surface_roughness", ra, "ra", ra),
        )
    )
    sections.append(("品質要求", body))
    dims = []
    for dim in rnd.sample(["length", "outer_diameter"], rnd.randint(1, 2)):
        nom, dp = nominal(rnd, dim)
        ns = f"{nom:.{dp}f}"
        traw, plus, minus = tol(rnd)
        lab = {"length": L(["全長", "長さ"]), "outer_diameter": L(["外径", "軸径"])}[dim]
        pre = "φ" if dim == "outer_diameter" and rnd.random() < 0.7 else ""
        dims.append((f"{lab} {pre}{ns} mm  {traw}", N(f"dimension.{dim}", ns, "mm", ns)))
        tr.append(TOL(plus, minus, traw))
    sections.append(("主要寸法", dims))
    sections.append(
        (
            "検査",
            [
                (f"抜取検査 AQL {rnd.choice(['0.65', '1.0', '0.4'])} により行う。", None),
                (f"検査成績書を納入毎に提出すること。(保管期間 {rnd.choice([3, 5, 10])}年)", None),
            ],
        )
    )
    sections.append(("梱包", [(f"{rnd.choice([20, 50, 100])}個/箱、防錆油塗布のこと。", None)]))
    num = rnd.choice(["{}.", "{})", "第{}条"])
    pages = [p]
    for si, (head, lines) in enumerate(sections, start=1):
        if y < 120 and not single:
            p = Pg()
            pages.append(p)
            y = 790
        p.t(40, y, 11, f"{num.format(si)} {head}")
        y -= 17
        for j, (text, truth) in enumerate(lines, start=1):
            p.t(58, y, 9, f"({j}) {text}" if len(lines) > 1 else text)
            if truth:
                tr.append(truth)
            y -= 15
        y -= 6
    for k, pg in enumerate(pages, start=1):
        pg.t(290, 30, 8, f"- {k} -")
    return pages, tr, []


# ---------------------------------------------------------------- 図面
ROLES = {
    "drawn": ["作成", "設計", "製図", "DRAWN"],
    "checked": ["検図", "照査", "審査", "CHECKED"],
    "approved": ["承認", "APPROVED", "APPD"],
}


def drawing(rnd: random.Random, single: bool = False) -> tuple[list[Pg], list[dict], list]:
    p, tr = Pg(), []
    L = rnd.choice  # noqa: N806
    # drawing area: views as dimension annotations
    dims = rnd.sample(
        ["outer_diameter", "inner_diameter", "length", "width", "thickness", "height"], rnd.randint(2, 4)
    )
    marks = {
        "outer_diameter": "φ",
        "inner_diameter": "φ",
        "length": "L=",
        "width": "W=",
        "thickness": "t=",
        "height": "H=",
    }
    table = rnd.random() < 0.4
    y = 760
    if table:
        p.t(330, 790, 9, rnd.choice(["主要寸法表", "寸法一覧 (mm)"]))
        for x, h in zip([330, 410, 480], ["寸法名", "基準寸法", "公差"], strict=True):
            p.t(x, 775, 8, h)
        y = 760
    for k, dim in enumerate(dims):
        nom, dp = nominal(rnd, dim)
        ns = f"{nom:.{dp}f}"
        has_tol = rnd.random() < 0.7
        traw, plus, minus = tol(rnd)
        if table:
            p.t(330, y, 8, L(DIM_LABELS[dim]))
            p.t(410, y, 8, ns)
            p.t(480, y, 8, traw if has_tol else "普通公差")
            y -= 14
        else:
            x, yy = 70 + (k % 2) * 150, 700 - (k // 2) * 90
            lab = marks[dim] if dim != "inner_diameter" else "穴 φ"
            p.t(
                x,
                yy,
                10,
                f"{lab}{ns}"
                + (f" ({traw})" if has_tol and rnd.random() < 0.5 else f" {traw}" if has_tol else ""),
            )
        tr.append(N(f"dimension.{dim}", ns, "mm", ns))
        if has_tol:
            tr.append(TOL(plus, minus, traw))
    if not table:
        p.t(250, 640, 9, rnd.choice(["C0.5", "R2", "2-M6 深10"]))  # chamfer/radius/thread distractor
        p.t(80, 560, 9, "A-A 断面")
    st = rnd.choice(TREATMENTS)
    notes = ["注記", "1. 指示なき角部は C0.3 とする。", "2. 普通公差は JIS B 0405 中級(m)による。"]
    st_in_notes = rnd.random() < 0.5
    if st_in_notes:
        notes.append(f"3. 表面処理：{st}")
    notes.append(f"{len(notes)}. バリ・カエリなきこと。")
    for j, line in enumerate(notes):
        p.t(40, 330 - j * 13, 8, line)
    # title block (bottom right) and approval box
    dn, pname, mat = dwg_no(rnd), rnd.choice(PARTS), rnd.choice(PART_MATS)
    rev = rnd.choice(["A", "B", "C", "D", "E", "1", "2"])
    scale = rnd.choice(["1:1", "1:2", "2:1", "1：5", "1:2.5"])
    ymd = rdate(rnd)
    ds = jdate(rnd, *ymd)
    names = rnd.sample(PEOPLE, 3)
    tr += [
        T("drawing_number", dn),
        T("part_name", pname),
        T("material", mat),
        T("revision", rev),
        T("scale", nk(scale), scale),
        D("approval_date", iso(ymd), ds),
        T("drawn_by", names[0]),
        T("checked_by", names[1]),
        T("approved_by", names[2]),
    ]
    if not st_in_notes:
        tr.append(T("surface_treatment", nk(st), st))
    tb = [
        (L(["図番", "図番 DWG.NO.", "DRAWING NO."]), dn),
        (L(["品名", "名称", "TITLE"]), pname),
        (L(["材質", "MATERIAL", "材料"]), mat),
        (L(["改訂", "REV.", "訂正"]), rev),
        (L(["尺度", "SCALE"]), scale),
        (L(["日付", "承認日", "DATE"]), ds),
    ]
    if not st_in_notes:
        tb.append((L(["表面処理", "処理", "FINISH"]), st))
    else:
        tr.append(T("surface_treatment", nk(st), st))
    tb.append(("投影法", "第三角法"))  # distractor
    rnd.shuffle(tb)
    style = rnd.randrange(3)
    x0, y0 = 300, 200
    for k, (lab, val) in enumerate(tb):
        x, yy = x0 + (k % 2) * 140, y0 - (k // 2) * 30
        if style == 0:  # label small above value
            p.t(x, yy + 9, 6, lab)
            p.t(x + 6, yy - 4, 10, val)
        elif style == 1:  # value first, label after it
            p.t(x, yy, 10, val)
            p.t(x + 90, yy, 6, lab)
        else:
            p.t(x, yy, 7, lab)
            p.t(x + 48, yy, 10, val)
    roles = {r: L(v) for r, v in ROLES.items()}
    bx, by = 40, 200
    seals = []
    for k, r in enumerate(("drawn", "checked", "approved")):
        x = bx + k * 70
        p.t(x + 10, by, 8, roles[r])
        p.t(x + 14, by - 30, 9, names[k])
        seals.append((x + 22, by - 27, names[k], r, roles[r]))
    if rnd.random() < 0.5:  # revision history: older dates, not the approval date
        old = rdate(rnd)
        p.t(40, 260, 7, f"△ {rev}  寸法変更  {old[0] - 1}/{old[1]}/{old[2]}")
    p.t(40, 120, 7, f"{rnd.choice(SUPPLIERS)}  TEL {rnd.choice(TELS)}")
    p.t(520, 40, 7, "1/1")
    p.t(300, 230, 7, rnd.choice(["単位 mm", "UNIT: mm", "寸法単位：ｍｍ"]))
    return [p], tr, seals


# ---------------------------------------------------------------- 注文書 CSV
def purchase_order(rnd: random.Random) -> tuple[bytes, list[dict]]:
    tr: list[dict] = []
    rows: list[list[str]] = []
    po = rnd.choice(
        [
            f"PO-{rnd.randint(2401, 2612)}-{rnd.randint(1, 999):04d}",
            f"{rnd.randint(240101, 261228)}-{rnd.randint(1, 99):03d}",
            f"HN{rnd.randint(1000, 9999)}",
        ]
    )
    ymd = rdate(rnd)
    csv_date = rnd.choice(
        [
            lambda y, m, d: f"{y}/{m:02d}/{d:02d}",
            lambda y, m, d: f"{y}-{m:02d}-{d:02d}",
            lambda y, m, d: f"{y}{m:02d}{d:02d}",
            lambda y, m, d: f"R{y - 2018}.{m}.{d}",
            lambda y, m, d: f"{y}年{m}月{d}日",
        ]
    )
    ds = csv_date(*ymd)
    sup = rnd.choice(SUPPLIERS)
    per_line_sup = rnd.random() < 0.6
    per_line_po = rnd.random() < 0.4
    per_line_issue = rnd.random() < 0.3
    pre = rnd.randrange(3)
    if pre == 0:
        rows += [["注文書"], ["発注番号", po], ["発注日", ds]]
        tr += [T("po_number", po), D("issue_date", iso(ymd), ds)]
        if not per_line_sup:
            rows.append(["発注先", f"{sup} 御中"])
            tr.append(T("supplier", nk(sup), sup))
        rows += [["発注元", rnd.choice(BUYERS)], ["TEL", rnd.choice(TELS)], []]
    elif pre == 1:
        rows.append([f"#{rnd.choice(['発注データ出力', 'EDI注文データ'])} {rnd.randint(1, 9)}版"])
        if not per_line_sup:
            rows.append([f"{sup} 様"])
            tr.append(T("supplier", nk(sup), sup))
        if not per_line_po:
            rows.append([f"注文No.:{po}"])
            tr.append(T("po_number", po))
        if not per_line_issue:
            rows.append(["発行日", ds])
            tr.append(D("issue_date", iso(ymd), ds))
    else:
        per_line_sup = per_line_po = per_line_issue = True
    cols = ["line", "pn", "name", "qty", "price", "due"] + (["sup"] if per_line_sup else [])
    cols += ["po"] if (per_line_po and pre != 0) else []
    cols += ["issue"] if (per_line_issue and pre != 0) else []
    cols += rnd.sample(["amount", "unit", "place", "memo"], rnd.randint(1, 3))
    rnd.shuffle(cols)
    if "line" in cols:
        cols.remove("line")
        cols.insert(0, "line")
    unit_in_head = rnd.random() < 0.5
    yen_head = rnd.random() < 0.5
    heads = {
        "line": rnd.choice(["行", "No.", "明細No.", "行番号"]),
        "pn": rnd.choice(["品番", "部品番号", "品目コード", "Item Code"]),
        "name": rnd.choice(["品名", "品目名称", "Description", "品名・規格"]),
        "qty": rnd.choice(["注文数", "数量", "発注数量", "Qty"]) + ("(個)" if unit_in_head else ""),
        "price": rnd.choice(["単価", "発注単価", "Unit Price"]) + ("(円)" if yen_head else ""),
        "due": rnd.choice(["納期", "納入期日", "希望納期", "Due Date"]),
        "sup": rnd.choice(["仕入先", "発注先", "Vendor", "取引先名"]),
        "po": rnd.choice(["注文番号", "PO No.", "発注No"]),
        "issue": rnd.choice(["発注日", "注文日"]),
        "amount": rnd.choice(["金額", "金額(税抜)", "Amount"]),
        "unit": "単位",
        "place": rnd.choice(["納入場所", "納入先"]),
        "memo": rnd.choice(["備考", "摘要"]),
    }
    rows.append([heads[c] for c in cols])
    step = rnd.choice([1, 10])
    total = 0
    for i in range(rnd.randint(2, 6)):
        q = rnd.choice([10, 20, 50, 100, 200, 500, 1000, 3000])
        price = rnd.choice([f"{rnd.randint(30, 4000)}", f"{rnd.randint(30, 900)}.{rnd.randint(0, 99):02d}"])
        due = (ymd[0] + (ymd[1] + 1) // 13, (ymd[1] % 12) + 1, rnd.randint(1, 28))
        dus = csv_date(*due)
        pn, pname = part_no(rnd), rnd.choice(PARTS)
        total += int(q * float(price))
        line = str((i + 1) * step)
        qs = f"{q:,}" if q >= 1000 and rnd.random() < 0.5 else str(q)
        ps = price if yen_head else rnd.choice([price, f"￥{price}", f"{price}円"])
        if "." not in price and int(price) >= 1000 and rnd.random() < 0.5:
            ps = ps.replace(price, f"{int(price):,}")
        unit_cell = rnd.choice(["個", "本", "pcs"])
        qcell = qs if unit_in_head or "unit" in cols or rnd.random() < 0.5 else qs + unit_cell
        q_unit = "pcs" if (unit_in_head or "unit" in cols or qcell != qs) else ""
        p_unit = "jpy" if yen_head or "￥" in ps or "円" in ps else ""
        praw = ps.replace("￥", "").replace("円", "")
        vals = {
            "line": line,
            "pn": pn,
            "name": pname,
            "qty": qcell,
            "price": ps,
            "due": dus,
            "sup": sup,
            "po": po,
            "issue": ds,
            "amount": f"{int(q * float(price)):,}",
            "unit": unit_cell,
            "place": rnd.choice(["第2工場", "本社倉庫", "刈谷物流センター"]),
            "memo": rnd.choice(["", "至急", "図面Rev.B", "分納可"]),
        }
        rows.append([vals[c] for c in cols])
        tr += [
            T("po_line", line),
            T("part_number", pn),
            T("part_name", pname),
            N("quantity", qs.replace(",", ""), q_unit, qs),
            N("unit_price", praw.replace(",", ""), p_unit, praw),
            D("delivery_date", iso(due), dus),
        ]
        if "sup" in cols:
            tr.append(T("supplier", nk(sup), sup))
        if "po" in cols:
            tr.append(T("po_number", po))
        if "issue" in cols:
            tr.append(D("issue_date", iso(ymd), ds))
    if rnd.random() < 0.6:
        rows.append(["", "合計", f"{total:,}"])
    if rnd.random() < 0.4:
        rows.append(["以上"])
    buf = io.StringIO()
    quoting = rnd.choice([csv.QUOTE_MINIMAL, csv.QUOTE_ALL])
    w = csv.writer(buf, quoting=quoting, lineterminator="\r\n")
    for r in rows:
        w.writerow(r)
    return buf.getvalue().encode("cp932"), tr


# ---------------------------------------------------------------- scans (own renderer)
SCAN_FONTS = ["Noto-Serif-CJK-JP", "Noto-Sans-Mono-CJK-JP", "Noto-Sans-CJK-JP"]


def scan_render(items: list[Item], seals: list, fmt: str, rnd: random.Random) -> bytes:
    dpi = rnd.choice([150, 150, 200, 240, 300])
    k = dpi / 72
    gray = rnd.random() < 0.3
    argv = [
        "magick",
        "-size",
        f"{int(PAGE_W * k)}x{int(PAGE_H * k)}",
        rnd.choice(["xc:white", "xc:#f7f5ef", "xc:#fbfbf8"]),
    ]
    argv += ["-font", rnd.choice(SCAN_FONTS), "-fill", "#111111"]

    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("%", "%%")

    seal_names = {s[2] for s in seals}
    for x, y, size, t in items:
        if t in seal_names:
            continue
        argv += [
            "-pointsize",
            f"{size * k:.1f}",
            "-annotate",
            f"+{int(x * k)}+{int((PAGE_H - y) * k)}",
            esc(t),
        ]
    for x, y, nm, *_ in seals:
        dx, dy = rnd.uniform(-6, 6), rnd.uniform(-4, 8)  # sometimes over neighbouring text
        cx, cy, r = int((x + dx) * k), int((PAGE_H - y - dy) * k), int(rnd.uniform(11, 14) * k)
        red = rnd.choice(["#d0201e", "#c8302a", "#e04040"])
        argv += ["-fill", "none", "-stroke", red, "-strokewidth", str(max(2, int(1.2 * k)))]
        if rnd.random() < 0.6:
            argv += ["-draw", f"circle {cx},{cy} {cx + r},{cy}"]
        else:
            argv += ["-draw", f"rectangle {cx - r},{cy - r} {cx + r},{cy + r}"]
        argv += ["-stroke", "none", "-fill", red, "-pointsize", f"{8 * k:.1f}"]
        argv += ["-annotate", f"+{cx - int(len(nm) * 4 * k)}+{cy + int(3 * k)}", esc(nm), "-fill", "#111111"]
    seed = rnd.randint(1, 10**6)
    argv += ["-colorspace", "Gray"] if gray else []
    argv += ["-seed", str(seed), "-blur", f"0x{rnd.uniform(0.3, 0.9):.2f}"]
    if rnd.random() < 0.5:
        argv += ["-attenuate", f"{rnd.uniform(0.15, 0.4):.2f}", "+noise", "Gaussian"]
    else:
        argv += ["-attenuate", f"{rnd.uniform(0.05, 0.15):.2f}", "+noise", "Impulse"]
    argv += ["-background", "white", "-rotate", f"{rnd.uniform(-2, 2):.2f}", "-alpha", "off"]
    argv += ["-colorspace", "Gray"] if gray else ["-colorspace", "sRGB"]
    argv += ["-depth", "8", "-strip", "-units", "PixelsPerInch", "-density", str(dpi)]
    if fmt == "pdf":
        argv += ["-quality", str(rnd.randint(60, 90)), "jpeg:-"]
        jpg = subprocess.run(argv, capture_output=True, check=True, timeout=180).stdout
        return jpeg_pdf(jpg, dpi)
    argv += ["png:-"]
    return subprocess.run(argv, capture_output=True, check=True, timeout=180).stdout


def jpeg_pdf(jpg: bytes, dpi: int) -> bytes:
    """Wraps one JPEG in a minimal PDF page (deterministic: no dates, no temp-file titles)."""
    i = 2
    while True:  # find the SOF marker for the pixel size
        marker, seglen = jpg[i + 1], int.from_bytes(jpg[i + 2 : i + 4], "big")
        if marker in (0xC0, 0xC1, 0xC2):
            h, w = int.from_bytes(jpg[i + 5 : i + 7], "big"), int.from_bytes(jpg[i + 7 : i + 9], "big")
            break
        i += 2 + seglen
    pw, ph = w * 72 / dpi, h * 72 / dpi
    content = f"q {pw:.2f} 0 0 {ph:.2f} 0 0 cm /Im0 Do Q".encode()
    cs = "/DeviceGray" if jpg[i + 9] == 1 else "/DeviceRGB"
    objs = [
        b"<< /Type /Catalog /Pages 2 0 R >>",
        b"<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        f"<< /Type /Page /Parent 2 0 R /MediaBox [0 0 {pw:.2f} {ph:.2f}] /Contents 4 0 R "
        f"/Resources << /XObject << /Im0 5 0 R >> >> >>".encode(),
        f"<< /Length {len(content)} >>\nstream\n".encode() + content + b"\nendstream",
        f"<< /Type /XObject /Subtype /Image /Width {w} /Height {h} /ColorSpace {cs} /BitsPerComponent 8 "
        f"/Filter /DCTDecode /Length {len(jpg)} >>\nstream\n".encode()
        + jpg
        + b"\nendstream",
    ]
    out = b"%PDF-1.4\n"
    offs = []
    for n, o in enumerate(objs, start=1):
        offs.append(len(out))
        out += f"{n} 0 obj\n".encode() + o + b"\nendobj\n"
    x = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    out += b"".join(f"{o:010d} 00000 n \n".encode() for o in offs)
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{x}\n%%EOF\n".encode()
    return out


BUILDERS = {
    "inspection_report": inspection,
    "certificate": certificate,
    "specification": specification,
    "drawing": drawing,
}


def _scan_docs(seed: int) -> list[Doc]:
    out = []
    for kind, fn in BUILDERS.items():
        for i in range(3):
            rnd = random.Random(f"{seed}:scan:{kind}:{i}")
            pages, truth, seals = fn(rnd, single=True)
            items = list(pages[0].items)
            if kind == "drawing":
                for *_xy, _nm, role, word in seals:
                    truth.append(T(f"stamp.{role}", "present", word))
            if rnd.random() < 0.5:
                ymd = rdate(rnd)
                hh, mm = rnd.randint(7, 19), rnd.randint(0, 59)
                stamp_raw = f"{ymd[0]}/{ymd[1]:02d}/{ymd[2]:02d} {hh:02d}:{mm:02d}"
                sender, num = rnd.choice(SUPPLIERS), rnd.choice(TELS)
                items.append((20, 830, 8, f"{stamp_raw}  FROM {sender}  FAX {num}  P.001/00{len(pages)}"))
                truth += [
                    D("fax.sent_at", f"{iso(ymd)}T{hh:02d}:{mm:02d}", stamp_raw),
                    T("fax.sender", nk(sender), sender),
                    T("fax.number", num),
                ]
            fmt = "png" if i % 2 == 0 else "pdf"
            data = scan_render(items, seals, fmt, rnd)
            out.append(
                Doc(
                    f"holdout/{kind}-scan-{i:02d}",
                    kind,
                    "image/png" if fmt == "png" else "application/pdf",
                    data,
                    truth,
                    "holdout_scan",
                    f"scan-{fmt}",
                    [items],
                    [(0, s[0], s[1], s[2]) for s in seals] if kind == "drawing" else [],
                )
            )
    return out


def build_holdout(seed: int = 7731, scans: bool = True) -> list[Doc]:
    docs: list[Doc] = []
    for kind, fn in BUILDERS.items():
        for i in range(8):
            rnd = random.Random(f"{seed}:{kind}:{i}")
            pages, truth, _seals = fn(rnd)
            per_char = i % 3 == 1
            items = [pg.items for pg in pages]
            docs.append(
                Doc(
                    f"holdout/{kind}-{i:02d}",
                    kind,
                    "application/pdf",
                    cjk_pdf(items, per_char),
                    truth,
                    "holdout",
                    "text-per-char" if per_char else "text",
                    items,
                    [],
                )
            )
    for i in range(8):
        data, truth = purchase_order(random.Random(f"{seed}:po:{i}"))
        docs.append(
            Doc(
                f"holdout/purchase_order-{i:02d}", "purchase_order", "text/csv", data, truth, "holdout", "csv"
            )
        )
    if scans and shutil.which("magick"):
        docs += _scan_docs(seed)
    return docs
