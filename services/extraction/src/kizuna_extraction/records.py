"""Turns positioned lines into records: `label: value` cells, key-value grids, pipe-delimited tables and
column-aligned tables (with two-row headers, item/role "inspection" tables and tables continuing on the next
page), plus FAX transmission headers.

Record = {label, value, location, group, method, confidence}. `location` is exact: page and line (1-based,
top-to-bottom), cell index within the line, and for tables the table/row/column index on that page. PDF lines
also carry a baseline bbox in PDF points, OCR lines a pixel bbox. Document text is only ever data: labels are
compared with a fixed vocabulary (claims.lookup) to recognise layouts, never interpreted otherwise.
"""

import re
from dataclasses import dataclass

from . import ja
from .claims import label_like, lookup, normalize_label
from .layout import Line, adjacent

LABEL_VALUE = re.compile(
    r"^\s*(?:\(?\d{1,2}[.)．、]\s+)?"  # optional enumerator: '1. ', '(2) '
    r"([^\W\d_][\w .#/()（）%\-]{0,40}?)"  # label
    r"\s*(?:[:=：]|…{2,}|・{3,}|\.{3,})\s*"  # separator, incl. leader dots '材質 ……… S45C'
    r"(\S.{0,200})$"
)
BRACKET_KV = re.compile(r"^\s*【([^】]{1,40})】\s*(\S.{0,200})$")
COLON_KV = re.compile(r"^\s*[^\W\d_][^:：]{0,40}[:：]\s*\S")
SEPARATOR = set("-:= +")

ITEM_HEADERS = {
    "検査項目",
    "項目",
    "測定項目",
    "特性",
    "管理項目",
    "item",
    "inspection item",
    "characteristic",
}
NOMINAL = {
    "規格値",
    "基準値",
    "基準寸法",
    "寸法",
    "図面寸法",
    "公称値",
    "規格",
    "nominal",
    "spec",
    "specification",
}
TOLERANCE = {"公差", "許容差", "tolerance", "tol"}
RESULT = {"判定", "結果", "result", "judgement", "judgment"}
MEASURED = re.compile(r"^(?:測定値|実測値|測定|x|n|no|#|sample|measured|actual)\s*[=.]?\s*\d{0,2}$")
ROLE_LABEL = {"nominal": "規格値", "tolerance": "公差", "measured": "測定値"}


class Sink:
    def __init__(self, limit: int):
        self.items: list[dict] = []
        self.limit = limit

    @property
    def full(self) -> bool:
        return len(self.items) >= self.limit

    def add(self, rec: dict) -> None:
        if not self.full:
            self.items.append(rec)


def _loc(line: Line, **kw) -> dict:
    loc: dict = {"page": line.page, "line": line.no, **kw}
    bbox = line.bbox()
    if bbox is not None:
        loc["bbox"] = bbox  # PDF points
    if "px_bbox" in line.extra:
        loc["bbox_px"] = line.extra["px_bbox"]  # OCR image pixels
    return loc


def kv_match(text: str) -> tuple[str, str] | None:
    m = LABEL_VALUE.match(text) or BRACKET_KV.match(text)
    return (m.group(1).strip(), m.group(2).strip()) if m else None


def _is_kv(text: str) -> bool:
    """For layout decisions only `label: value` with a colon counts ('n=1' is a column header, not a pair)."""
    return COLON_KV.match(text) is not None or BRACKET_KV.match(text) is not None


# Which layout reader produced a value (`location.layout`): the calibration groups values by it, because the
# geometric readers (strips, title blocks, horizontal tables) pair label and value by position only.
LAYOUTS = {"text": "kv", "table": "table", "htable": "htable", "pair": "pair", "title-block": "title"}


def _rec(label: str, value: str, loc: dict, group: str, method: str, conf: float) -> dict:
    if method.startswith("ocr"):
        loc = {**loc, "ocr_confidence": round(conf, 3)}  # travels with the value into the claim's location
    layout = LAYOUTS.get(method.split("-", 1)[1]) if "-" in method else None
    if layout is not None:
        loc = {**loc, "layout": layout}
    return {
        "label": label[:120],
        "value": value[:500],
        "location": loc,
        "group": group,
        "method": method,
        "confidence": round(conf, 3),
    }


def _numeric(text: str) -> bool:
    return ja.parse_quantity(text) is not None or ja.parse_tolerance(text) is not None


def _numeric_row(ln: Line) -> bool:
    return sum(_numeric(c.text) for c in ln.cells) * 2 >= len(ln.cells)


def _known(text: str) -> bool:
    return lookup(normalize_label(text)[0]) is not None


def _kv_grid(ln: Line) -> bool:
    """Title-block rows like `図番 | DW-1 | 品名 | ブラケット`: label, value, label, value...
    Labels are in the vocabulary or have a label ending; values are neither."""
    cs = ln.cells
    if len(cs) < 2 or len(cs) % 2:
        return False
    labs, vals = cs[0::2], cs[1::2]
    return all(label_like(c.text) for c in labs) and not any(
        _known(c.text) or label_like(c.text) for c in vals
    )


@dataclass
class Header:
    line: Line
    labels: list[str]  # per column; super-header units applied
    roles: list[
        str | None
    ]  # item/role table: "item" | "nominal" | "tolerance" | "measured" | "result" | None
    unit: str | None  # unit of the nominal column, used for measured values of an item/role table

    @property
    def pivot(self) -> bool:
        return "item" in self.roles


def _role(text: str) -> str | None:
    base, _unit = normalize_label(text, translate=False)
    if base in ITEM_HEADERS:
        return "item"
    if base in NOMINAL:
        return "nominal"
    if base in TOLERANCE:
        return "tolerance"
    if base in RESULT:
        return "result"
    if MEASURED.match(base):
        return "measured"
    return None


def _header(line: Line, sup: Line | None) -> Header:
    labels = []
    for c in line.cells:
        text = c.text
        if sup is not None and "(" not in ja.nfkc(text):
            owners = [s for s in sup.cells if s.x0 <= c.x0 + max(2.0, 0.25 * (s.x1 - s.x0))]
            unit = normalize_label(owners[-1].text)[1] if owners else None
            if unit:
                text = f"{text} ({unit})"
        labels.append(text)
    roles = [_role(t) for t in labels]
    if roles[0] != "item" or not any(r in ("nominal", "measured", "tolerance") for r in roles):
        roles = [None] * len(labels)
    unit = next((normalize_label(t)[1] for t, r in zip(labels, roles, strict=True) if r == "nominal"), None)
    return Header(line, labels, roles, unit)


def _nearest(header: Line, x0: float, x1: float) -> int:
    mid = (x0 + x1) / 2
    best = min(
        range(len(header.cells)), key=lambda i: abs((header.cells[i].x0 + header.cells[i].x1) / 2 - mid)
    )
    return best + 1


def _table_row(hdr: Header, ln: Line, page: int, table_no: int, row_no: int, method: str, sink: Sink) -> None:
    group = f"p{page}t{table_no}r{row_no}"
    cells = [(_nearest(hdr.line, c.x0, c.x1), c) for c in ln.cells]
    item = next((c.text for col, c in cells if hdr.roles[col - 1] == "item"), None)
    for col, c in cells:
        conf = (c.conf if c.conf is not None else ln.confidence) * 0.9
        label = hdr.labels[col - 1]
        role = hdr.roles[col - 1]
        loc = _loc(ln, table=table_no, row=row_no, col=col)
        if role == "item":
            continue
        if role is not None and item:
            loc |= {"header": label[:60], "item": item[:60]}
            if role == "result":
                label = "判定"
            else:
                unit = hdr.unit if role != "tolerance" and "(" not in ja.nfkc(item) else None
                label = f"{item} {ROLE_LABEL[role]}" + (f" ({unit})" if unit else "")
        sink.add(_rec(label, c.text, loc, group, f"{method}-table", conf))


def records_from_lines(lines: list[Line], method: str, sink: Sink) -> None:
    by_page: dict[int, list[Line]] = {}
    for ln in lines:
        by_page.setdefault(ln.page, []).append(ln)
    carry: list[Header | None] = [None]  # a table header that may continue on the next page
    for page, plines in by_page.items():
        if page == min(by_page):
            _fax(plines[:3], method, sink)
        _page(page, plines, method, sink, carry)
        if sink.full:
            return


def _fax(lines: list[Line], method: str, sink: Sink) -> None:
    for ln in lines:
        h = ja.fax_header(ln.text)
        if h is None:
            continue
        ln.extra["fax_header"] = True
        for key, label in (("sender", "fax sender"), ("number", "fax number"), ("sent_at", "fax sent at")):
            if key in h:
                sink.add(_rec(label, h[key], _loc(ln), "fax_header", f"{method}-fax-header", ln.confidence))
        return


def _page(page: int, lines: list[Line], method: str, sink: Sink, carry: list[Header | None]) -> None:
    from .geometry import emit_strip, horizontal_tables, strip

    used, table_no = horizontal_tables(lines, page, method, sink, 0)
    for i in used:
        lines[i].extra["read"] = True  # structure lines: later readers (title blocks) leave them alone
    pipe_header: list[str] | None = None
    grid: Header | None = None
    skip_until = 0
    row_no = 0
    for i, ln in enumerate(lines):
        if sink.full:
            return
        if i < skip_until or i in used or ln.extra.get("fax_header"):
            if i in used:
                grid = None
            continue
        text = ln.text.strip()
        conf = ln.confidence
        # 1. pipe-delimited tables (markdown style)
        if text.startswith("|") and text.endswith("|") and text.count("|") >= 3:
            cells = [c.strip() for c in text.strip("|").split("|")]
            if all(set(c) <= SEPARATOR for c in cells):
                continue
            if pipe_header is None:
                pipe_header, table_no, row_no = cells, table_no + 1, 0
                continue
            row_no += 1
            for col, (h, v) in enumerate(zip(pipe_header, cells, strict=False), start=1):
                if h and v:
                    loc = _loc(ln, table=table_no, row=row_no, col=col)
                    sink.add(_rec(h, v, loc, f"p{page}t{table_no}r{row_no}", f"{method}-table", conf * 0.95))
            continue
        pipe_header = None
        # 2. key-value grid rows (title blocks): pairs, not a table
        if _kv_grid(ln):
            grid = None
            for cno in range(0, len(ln.cells), 2):
                lab, val = ln.cells[cno], ln.cells[cno + 1]
                c = min(x for x in (lab.conf, val.conf, conf) if x is not None)
                sink.add(_rec(lab.text, val.text, _loc(ln, cell=cno + 2), "doc", f"{method}-text", c))
            continue
        # 3. label/value strips: labels with their values directly below (or above)
        nxt = lines[i + 1] if i + 1 < len(lines) else None
        if grid is None and nxt is not None and (i + 1) not in used:
            pair = strip(ln, nxt)
            if pair is not None:
                emit_strip(*pair, method, sink)
                ln.extra["read"] = nxt.extra["read"] = True
                skip_until = i + 2
                continue
        # 4. column-aligned tables: consecutive multi-cell lines without `label: value` cells
        gridish = len(ln.cells) >= 2 and not any(_is_kv(c.text) for c in ln.cells)
        if gridish and grid is not None and adjacent(lines[i - 1], ln):
            row_no += 1
            _table_row(grid, ln, page, table_no, row_no, method, sink)
            carry[0] = grid
            continue
        if gridish and grid is None and table_no == 0 and carry[0] is not None and _numeric_row(ln):
            # first rows of a page continuing the previous page's table (header not repeated)
            grid, table_no, row_no = carry[0], 1, 1
            _table_row(grid, ln, page, table_no, row_no, method, sink)
            continue
        nxt = lines[i + 1] if i + 1 < len(lines) else None
        nxt2 = lines[i + 2] if i + 2 < len(lines) else None

        def gridline(a: Line, b: Line | None) -> bool:
            return (
                b is not None
                and adjacent(a, b)
                and len(b.cells) >= 2
                and not any(_is_kv(c.text) for c in b.cells)
            )

        if gridish and gridline(ln, nxt):
            assert nxt is not None
            if not _numeric_row(nxt) and gridline(nxt, nxt2) and nxt2 is not None and _numeric_row(nxt2):
                grid = _header(nxt, ln)  # two-row header: `ln` is a super-header (units, groups)
                ln.extra["read"] = nxt.extra["read"] = True
                skip_until = i + 2
            else:
                grid = _header(ln, None)
                ln.extra["read"] = True
            table_no, row_no = table_no + 1, 0
            carry[0] = grid
            continue
        grid = None
        # 5. `label: value` cells (a line may hold several, e.g. title blocks)
        for cno, c in enumerate(ln.cells, start=1):
            m = kv_match(c.text)
            if m:
                loc = _loc(ln, cell=cno) if len(ln.cells) > 1 else _loc(ln)
                cc = c.conf if c.conf is not None else conf
                sink.add(_rec(m[0], m[1], loc, "doc", f"{method}-text", cc))
