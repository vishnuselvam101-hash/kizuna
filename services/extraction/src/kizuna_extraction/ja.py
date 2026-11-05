"""Japanese document normalisation: full-width text, 和暦 dates, numbers with units, tolerances, results, FAX
transmission headers. Pure functions over bounded strings; anything they cannot parse exactly returns None
(the original text is always kept by the caller, never replaced)."""

import re
import unicodedata
from decimal import Decimal, InvalidOperation

MAX_IN = 200  # longer values are not parsed as numbers/dates (they stay text)

# Minus/dash look-alikes seen in JP documents (U+2212 minus, U+2010..2015 dashes, U+FF0D full-width hyphen).
DASHES = dict.fromkeys(map(ord, "−‐‑‒–—―－﹣"), "-")
WAVES = dict.fromkeys(map(ord, "〜～"), "~")  # 〜 ～


def nfkc(s: str) -> str:
    """NFKC (full-width ASCII/digits → ASCII, ㎜ → mm, ² → 2) plus dash/wave look-alikes; spaces collapsed."""
    return " ".join(unicodedata.normalize("NFKC", s).translate(DASHES).translate(WAVES).split())


def is_cjk(ch: str) -> bool:
    return unicodedata.east_asian_width(ch) in ("W", "F")


def join_cjk(a: str, b: str) -> str:
    """Joins two adjacent text pieces; no space when either boundary character is CJK (OCR, per-glyph PDF)."""
    if a and b and (is_cjk(a[-1]) or is_cjk(b[0])):
        return a + b
    return f"{a} {b}" if a else b


# ------------------------------------------------------------------ dates
ERAS = {  # name → (first Gregorian year - 1, first valid date, last valid date)
    "令和": (2018, (2019, 5, 1), (9999, 12, 31)),
    "平成": (1988, (1989, 1, 8), (2019, 4, 30)),
    "昭和": (1925, (1926, 12, 25), (1989, 1, 7)),
}
ERA_LETTER = {"R": "令和", "H": "平成", "S": "昭和"}
WAREKI = re.compile(r"^(令和|平成|昭和)\s*(元|\d{1,2})\s*年\s*(\d{1,2})\s*月\s*(\d{1,2})\s*日?$")
WAREKI_SHORT = re.compile(r"^([RHS])\s*(\d{1,2})\s*[./-]\s*(\d{1,2})\s*[./-]\s*(\d{1,2})$", re.I)
SEIREKI = re.compile(
    r"^(\d{4})\s*(?:年\s*(\d{1,2})\s*月\s*(\d{1,2})\s*日?|[./-]\s*(\d{1,2})\s*[./-]\s*(\d{1,2}))$"
)


def _valid(y: int, m: int, d: int) -> bool:
    if not (1900 <= y <= 2200 and 1 <= m <= 12 and d >= 1):
        return False
    leap = y % 4 == 0 and (y % 100 != 0 or y % 400 == 0)
    days = [31, 29 if leap else 28, 31, 30, 31, 30, 31, 31, 30, 31, 30, 31][m - 1]
    return d <= days


def parse_date(text: str) -> str | None:
    """'令和7年4月1日', '令和元年5月1日', 'R7.4.1', 'H31/4/30', '2025年4月1日', '2025/4/1' → ISO 'YYYY-MM-DD'.
    Dates outside their era (令和1年4月1日, 平成32年) are refused, as are impossible calendar dates."""
    if len(text) > 40:
        return None
    t = nfkc(text)
    m = WAREKI.match(t)
    era = None
    if m:
        era, ey, mo, d = m.group(1), m.group(2), m.group(3), m.group(4)
    else:
        m = WAREKI_SHORT.match(t)
        if m:
            era, ey, mo, d = ERA_LETTER[m.group(1).upper()], m.group(2), m.group(3), m.group(4)
    if era is not None:
        base, first, last = ERAS[era]
        y = base + (1 if ey == "元" else int(ey))
        ymd = (y, int(mo), int(d))
        if not _valid(*ymd) or not first <= ymd <= last:
            return None
        return f"{ymd[0]:04d}-{ymd[1]:02d}-{ymd[2]:02d}"
    m = SEIREKI.match(t)
    if not m:
        return None
    y = int(m.group(1))
    mo, d = (m.group(2), m.group(3)) if m.group(2) else (m.group(4), m.group(5))
    return f"{y:04d}-{int(mo):02d}-{int(d):02d}" if _valid(y, int(mo), int(d)) else None


# ------------------------------------------------------------------ numbers and units
UNITS = {  # normalised spelling → canonical unit
    "mm": "mm",
    "cm": "cm",
    "m": "m",
    "in": "in",
    "um": "um",
    "µm": "um",
    "μm": "um",
    "kg": "kg",
    "g": "g",
    "n": "n",
    "mpa": "mpa",
    "n/mm2": "mpa",
    "n/mm^2": "mpa",
    "gpa": "gpa",
    "kgf/mm2": "kgf/mm2",
    "hrc": "hrc",
    "hrb": "hrb",
    "hv": "hv",
    "hb": "hb",
    "ra": "ra",
    "rz": "rz",
    "%": "%",
    "pcs": "pcs",
    "pc": "pcs",
    "個": "pcs",
    "本": "pcs",
    "枚": "pcs",
    "ヶ": "pcs",
    "円": "jpy",
    "jpy": "jpy",
    "¥": "jpy",
    "yen": "jpy",
}
PREFIX_UNITS = {"hrc", "hrb", "hv", "hb", "ra", "rz", "¥"}  # 'HRC55', 'Ra 1.6', '¥1,250'
DIAMETER = "φΦ∅Ø⌀"
NUMBER = re.compile(r"^[-+]?(?:\d{1,3}(?:,\d{3})+|\d+)(?:\.\d+)?$")
QTY = re.compile(r"^([^\d\s+-]{0,4}?)\s*([-+]?[\d,]*\d(?:\.\d+)?)\s*([^\d\s][^\s\d]{0,8}\d?)?$")


def unit_of(u: str | None) -> str | None:
    """Canonical unit for a unit string ('N/mm²' → 'mpa', '㎜' → 'mm', '個' → 'pcs'); None when unknown."""
    if not u:
        return None
    k = nfkc(u).lower().replace(" ", "").replace("²", "2")
    return UNITS.get(k)


def number(s: str) -> str | None:
    """'1,200' → '1200' (comma = thousands separator only in groups of three), '３．２０' → '3.20'."""
    t = nfkc(s).replace(" ", "")
    if not NUMBER.match(t):
        return None
    try:
        Decimal(t.replace(",", ""))
    except InvalidOperation:
        return None
    return t.replace(",", "").lstrip("+")


def parse_quantity(value: str, label_unit: str | None = None) -> tuple[str, str | None] | None:
    """'120.5㎜', 'HRC55', 'Ra 1.6', '￥1,250', '1,200 個', 'φ25', '440 N/mm²' → (number, unit).
    A unit that is present but unknown makes the whole value unparsed (None)."""
    if len(value) > MAX_IN:
        return None
    t = nfkc(value).lstrip(DIAMETER).strip()
    m = QTY.match(t)
    if not m:
        return None
    pre, num, post = m.group(1).strip(), number(m.group(2)), (m.group(3) or "").strip()
    if num is None or (pre and post):
        return None
    if pre:
        u = unit_of(pre)
        if u is None or (u not in PREFIX_UNITS and pre != "¥"):
            return None
        return num, u
    if post:
        u = unit_of(post)
        return (num, u) if u is not None else None
    return num, unit_of(label_unit) or (nfkc(label_unit).lower() if label_unit else None)


TOL_SYM = re.compile(r"^(?:±|\+/-|\+-)\s*(\d+(?:\.\d+)?)\s*([a-z%]*)$", re.I)
TOL_ASYM = re.compile(r"^\+?\s*(\d+(?:\.\d+)?)\s*(?:/|\s)\s*-\s*(\d+(?:\.\d+)?)\s*([a-z%]*)$", re.I)
TOL_ASYM_REV = re.compile(r"^-\s*(\d+(?:\.\d+)?)\s*(?:/|\s)\s*\+\s*(\d+(?:\.\d+)?)\s*([a-z%]*)$", re.I)


def parse_tolerance(value: str) -> tuple[str, str, str | None] | None:
    """'±0.05', '+0.1/−0', '＋０．１／－０', '+0.1 -0.2', '-0.05/+0', '0/-0.05' → (plus, minus, unit)."""
    if len(value) > 60:
        return None
    t = nfkc(value).replace(" / ", "/")
    m = TOL_SYM.match(t)
    if m:
        return m.group(1), m.group(1), unit_of(m.group(2)) if m.group(2) else None
    m = TOL_ASYM.match(t)
    if m and (t.startswith("+") or t.startswith("0")):
        return m.group(1), m.group(2), unit_of(m.group(3)) if m.group(3) else None
    m = TOL_ASYM_REV.match(t)
    if m:
        return m.group(2), m.group(1), unit_of(m.group(3)) if m.group(3) else None
    return None


INLINE_TOL = re.compile(r"^(.*?\d)\s*([a-zµμ%²/]*)\s*((?:±|\+/-|\+)\s*\d.*)$", re.I)


def split_inline_tolerance(value: str) -> tuple[str, str] | None:
    """'120.5㎜ ±0.1' → ('120.5 mm', '±0.1'); 'φ25 +0/−0.05' → ('φ25', '+0/−0.05'); else None."""
    if len(value) > 80:
        return None
    m = INLINE_TOL.match(ja_text := nfkc(value))
    if not m:
        return None
    head = f"{m.group(1)} {m.group(2)}".strip()
    tol = m.group(3).strip()
    if parse_tolerance(tol) is None or parse_quantity(head) is None or len(ja_text) > 80:
        return None
    return head, tol


RESULTS = {
    "pass": "pass",
    "ok": "pass",
    "accept": "pass",
    "accepted": "pass",
    "good": "pass",
    "合格": "pass",
    "良": "pass",
    "○": "pass",
    "〇": "pass",
    "◯": "pass",
    "fail": "fail",
    "ng": "fail",
    "nok": "fail",
    "reject": "fail",
    "rejected": "fail",
    "不合格": "fail",
    "否": "fail",
    "×": "fail",
    "✕": "fail",
}


def result(value: str) -> str | None:
    return RESULTS.get(nfkc(value).lower()) if len(value) <= 20 else None


APPROVAL = {
    "approved": "approved",
    "released": "approved",
    "済": "approved",
    "承認済": "approved",
    "承認済み": "approved",
    "承認": "approved",
    "pending": "pending",
    "draft": "pending",
    "未": "pending",
    "未承認": "pending",
    "rejected": "rejected",
    "否認": "rejected",
}


def approval(value: str) -> str | None:
    return APPROVAL.get(nfkc(value).lower()) if len(value) <= 20 else None


# ------------------------------------------------------------------ FAX transmission header
FAX_TIME = re.compile(
    r"(\d{4})\s*[/.-]\s*(\d{1,2})\s*[/.-]\s*(\d{1,2})\s*(?:\([^)]{0,3}\))?\s+(\d{1,2})\s*:\s*(\d{2})"
)
FAX_FROM = re.compile(r"(?<![a-z])FROM\s*[:：]?\s*(.+?)(?=\s{2,}|\s*(?:FAX|TEL)|\s+P\.?\s*\d|$)", re.I)
FAX_NO = re.compile(r"(?<![a-z])FAX\s*(?:NO\.?)?\s*[:：]?\s*(\+?[\d()][\d() -]{7,18}\d)", re.I)


def fax_header(line: str) -> dict[str, str] | None:
    """Parses a fax machine's transmission header ('2025/04/01 10:23 FROM:株式会社X FAX:03-1234-5678 P.001').
    Needs the timestamp and at least one of sender / number; returns {sent_at, sender?, number?}."""
    if len(line) > 300:
        return None
    t = nfkc(line)
    tm = FAX_TIME.search(t)
    if not tm:
        return None
    y, mo, d, hh, mi = (int(g) for g in tm.groups())
    if not _valid(y, mo, d) or hh > 23 or mi > 59:
        return None
    out = {"sent_at": f"{y:04d}-{mo:02d}-{d:02d}T{hh:02d}:{mi:02d}"}
    fm = FAX_FROM.search(t)
    if fm and fm.group(1).strip():
        out["sender"] = fm.group(1).strip()[:120]
    nm = FAX_NO.search(t)
    if nm:
        out["number"] = re.sub(r"\s+", "", nm.group(1))
    return out if len(out) > 1 else None
