"""Layout readers that work on text positions (and ruling lines when the PDF draws them):

- horizontal measurement tables: inspection items as columns, one row per role (規格値, 公差, 測定値, 判定);
  columns come from the ruling lines around the item cells, else from the item cells' positions; a value
  cell spanning several columns (merged cell) applies to each; measured rows may share one vertically merged
  label (`測定値` once, then `2`, `3`, or nothing);
- label/value strips: a row of labels with the values directly below (or the values directly above), paired by
  horizontal overlap;

Part sections (multi-part documents) are resolved by the Rust mapper from these records' page/line positions.

Document text is only ever data: labels are compared with the fixed vocabulary.
"""

from __future__ import annotations

import re

from . import ja
from .claims import label_like, lookup, normalize_label
from .layout import Cell, Line
from .records import ROLE_LABEL, Sink, _loc, _rec, _role

UNIT_ROW = {"単位", "unit", "units"}
SUB_INDEX = re.compile(r"^(?:n\s*=\s*)?\(?\d{1,2}\)?$|^[xX#]\d{1,2}$")
PAIR_GAP = 2.6  # label and value lines at most this many line heights apart


def _near(a: Line, b: Line, k: float = PAIR_GAP) -> bool:
    if a.page != b.page or a.y is None or b.y is None:
        return a.page == b.page and b.no == a.no + 1
    return 0 < a.y - b.y <= k * max(a.size, b.size, 1.0)


def _row_label(text: str) -> str | None:
    """Role of a horizontal table's row label: item, nominal, tolerance, measured, result or unit."""
    base = normalize_label(text, translate=False)[0]
    if base in UNIT_ROW:
        return "unit"
    if base in ("judge", "合否判定", "可否"):
        return "result"
    return _role(text)


def _columns(header: Line, items: list[Cell], rules: list) -> list[tuple[float, float]]:
    """x-interval of each item column: between the vertical rules around the item cell when the PDF draws
    them, else halfway to the neighbouring item cells."""
    y = header.y
    verts = sorted(
        {round(r[0], 1) for r in rules if r[0] == r[2] and y is not None and r[1] - 2 <= y <= r[3] + 2}
    )
    out: list[tuple[float, float]] = []
    for i, c in enumerate(items):
        mid = (c.x0 + c.x1) / 2
        left = max((v for v in verts if v <= c.x0 + 1), default=None)
        right = min((v for v in verts if v >= c.x1 - 1), default=None)
        if left is not None and right is not None and left < mid < right:
            out.append((left, right))
            continue
        lo = (items[i - 1].x1 + c.x0) / 2 if i else c.x0 - (c.x1 - c.x0)
        hi = (c.x1 + items[i + 1].x0) / 2 if i + 1 < len(items) else c.x1 + max(40.0, c.x1 - c.x0)
        out.append((lo, hi))
    return out


def _item_name(text: str) -> str:
    """An item cell's name; a note in brackets that is not a unit (`外径（図面指示）`) is dropped."""
    base, unit = normalize_label(text, translate=False)
    if unit and ja.parse_quantity(f"1 {unit}") is None:
        return base
    return text


def _cover(c: Cell, cols: list[tuple[float, float]]) -> list[int]:
    """Columns a value cell belongs to: the one holding its centre, or every column whose centre it spans."""
    spanned = [j for j, (lo, hi) in enumerate(cols) if c.x0 <= (lo + hi) / 2 <= c.x1]
    if len(spanned) > 1:
        return spanned
    mid = (c.x0 + c.x1) / 2
    return [j for j, (lo, hi) in enumerate(cols) if lo <= mid < hi][:1]


def _plain(c: Cell) -> bool:
    return (
        _row_label(c.text) is None
        and ja.parse_quantity(c.text) is None
        and not any(ch in c.text for ch in ":：")
    )


def _htable_items(ln: Line, nxt: Line | None) -> list[Cell] | None:
    """Item cells of a horizontal table header: `検査項目 | 外径 | 内径 ...`, or the item names alone above a
    row that starts with a role label left of them (`規格値 | 25.00 | 12.00`)."""
    cs = ln.cells
    if len(cs) >= 3 and _row_label(cs[0].text) == "item" and all(_plain(c) for c in cs[1:]):
        return cs[1:]
    if len(cs) >= 2 and nxt is not None and nxt.cells and all(_plain(c) for c in cs):
        head = nxt.cells[0]
        if _row_label(head.text) in ("nominal", "tolerance", "measured", "unit") and head.x1 <= cs[0].x0 + 1:
            return cs
    return None


def horizontal_tables(
    plines: list[Line], page: int, method: str, sink: Sink, table_no: int
) -> tuple[set[int], int]:
    """Reads horizontal tables on one page. Returns (indexes of consumed lines, tables read)."""
    used: set[int] = set()
    i = 0
    while i < len(plines):
        hdr = plines[i]
        nxt = plines[i + 1] if i + 1 < len(plines) and _near(hdr, plines[i + 1], 2.2) else None
        item_cells = _htable_items(hdr, nxt)
        if item_cells is None:
            i += 1
            continue
        cols = _columns(hdr, item_cells, hdr.extra.get("rules") or [])
        items = [_item_name(c.text) for c in item_cells]
        first_x = min(lo for lo, _hi in cols)
        rows: list[tuple[int, str, str, list[tuple[int, Cell]]]] = []  # (line idx, role, label, cells)
        prev, last = hdr, None
        units: list[str | None] = [None] * len(items)
        skipped = False
        j = i + 1
        while j < len(plines) and _near(prev, plines[j], 2.2):
            ln = plines[j]
            is_lab = [c.x1 <= first_x + 1 or (c.x0 + c.x1) / 2 < item_cells[0].x0 for c in ln.cells]
            labs = [c for c, lb in zip(ln.cells, is_lab, strict=True) if lb]
            vals = [c for c, lb in zip(ln.cells, is_lab, strict=True) if not lb]
            lab_text = " ".join(c.text for c in labs)
            head = labs[0].text if labs else ""
            role = _row_label(head) if labs else None
            if (
                role is None
                and last == "measured"
                and (not labs or all(SUB_INDEX.match(c.text) for c in labs))
            ):
                role = "measured"  # vertically merged 測定値 label: `2`, `3`, or no label at all
            if (
                role is None
                and vals
                and not skipped
                and j + 1 < len(plines)
                and _near(ln, plines[j + 1], 2.2)
            ):
                skipped = True  # one row of another kind inside the table (規格範囲, 上限/下限): not read
                used.add(j)
                prev, last = ln, None
                j += 1
                continue
            if role in (None, "item") or not vals:
                break
            cells = [(k, c) for c in vals for k in _cover(c, cols)]
            if role == "unit":
                for k, c in cells:
                    units[k] = c.text
            else:
                rows.append((j, role, lab_text, cells))
            used.add(j)
            prev, last = ln, role
            j += 1
        if not any(r[1] in ("nominal", "tolerance", "measured") for r in rows):
            i += 1
            continue
        used.add(i)
        table_no += 1
        nom_unit = next((normalize_label(r[2])[1] for r in rows if r[1] == "nominal"), None)
        for rno, (li, role, lab_text, cells) in enumerate(rows, start=1):
            ln = plines[li]
            for k, c in cells:
                item = items[k]
                unit = units[k] or nom_unit
                if role == "result":
                    label = "判定"
                else:
                    has_unit = "(" in ja.nfkc(item)
                    u = unit if role != "tolerance" and not has_unit else None
                    label = f"{item} {ROLE_LABEL[role]}" + (f" ({u})" if u else "")
                loc = _loc(ln, table=table_no, row=rno, col=k + 1) | {
                    "header": lab_text[:60],
                    "item": item[:60],
                }
                conf = (c.conf if c.conf is not None else ln.confidence) * 0.9
                sink.add(_rec(label, c.text, loc, f"p{page}t{table_no}c{k + 1}", f"{method}-htable", conf))
        i = j
    return used, table_no


# ---------------------------------------------------------------- label / value strips
def _is_label(c: Cell) -> bool:
    t = c.text.strip()
    if len(t) > 24 or any(ch in t for ch in ":："):
        return False
    return label_like(t)


def _is_value(c: Cell) -> bool:
    t = c.text.strip()
    return bool(t) and not _is_label(c) and not any(ch in t for ch in ":：")


def _overlap(a: Cell, b: Cell) -> float:
    return min(a.x1, b.x1) - max(a.x0, b.x0)


def _pairs(labels: Line, values: Line) -> list[tuple[int, Cell, int, Cell]] | None:
    """Each value cell paired with the label cell it overlaps most (or the nearest within a cell width)."""
    out = []
    taken: set[int] = set()
    for vi, v in enumerate(values.cells):
        best, score = None, None
        for li, lc in enumerate(labels.cells):
            if li in taken:
                continue
            ov = _overlap(lc, v)
            dist = abs(lc.x0 - v.x0)
            ok = ov > 0 or dist <= max(12.0, 0.6 * (lc.x1 - lc.x0))
            if ok and (score is None or (ov, -dist) > score):
                best, score = li, (ov, -dist)
        if best is None:
            return None
        taken.add(best)
        out.append((best, labels.cells[best], vi, v))
    return out


def strip(a: Line, b: Line) -> tuple[Line, Line] | None:
    """(labels line, values line) when `a` and `b` are a label/value strip: one line of labels and, directly
    below or above it, one line of values aligned with them. A single label needs to be in the vocabulary."""
    if not _near(a, b):
        return None
    for labs, vals in ((a, b), (b, a)):
        if not all(_is_label(c) for c in labs.cells) or not all(_is_value(c) for c in vals.cells):
            continue
        if len(vals.cells) > len(labs.cells):
            continue
        if len(labs.cells) == 1 and lookup(normalize_label(labs.cells[0].text)[0]) is None:
            continue
        if any(_role(c.text) for c in labs.cells):
            continue  # an item/role table header, not a strip
        if _pairs(labs, vals) is not None:
            return labs, vals
    return None


def emit_strip(labs: Line, vals: Line, method: str, sink: Sink) -> None:
    for _li, lc, vi, v in _pairs(labs, vals) or []:
        conf = min(x for x in (lc.conf, v.conf, vals.confidence) if x is not None)
        loc = _loc(vals, cell=vi + 1) if len(vals.cells) > 1 else _loc(vals)
        sink.add(_rec(lc.text, v.text, loc, "doc", f"{method}-pair", conf))
