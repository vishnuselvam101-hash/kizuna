"""Turns positioned lines into records: `label: value` cells, pipe-delimited tables and column-aligned tables.

Record = {label, value, location, group, method, confidence}. `location` is exact: page and line (1-based,
top-to-bottom), cell index within the line, and for tables the table/row/column index on that page. PDF lines
also
carry a baseline bbox in PDF points. Document text is only ever data: nothing here interprets it.
"""

import re

from .layout import Line, adjacent

LABEL_VALUE = re.compile(r"^\s*([^\W\d_][\w .#/()\-]{0,40}?)\s*[:=：]\s*(\S.{0,200})$")
SEPARATOR = set("-:= +")


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


def _is_kv(text: str) -> bool:
    return LABEL_VALUE.match(text) is not None


def _rec(label: str, value: str, loc: dict, group: str, method: str, conf: float) -> dict:
    return {
        "label": label[:120],
        "value": value[:500],
        "location": loc,
        "group": group,
        "method": method,
        "confidence": round(conf, 3),
    }


def _nearest(header: Line, x0: float, x1: float) -> int:
    mid = (x0 + x1) / 2
    best = min(
        range(len(header.cells)), key=lambda i: abs((header.cells[i].x0 + header.cells[i].x1) / 2 - mid)
    )
    return best + 1


def records_from_lines(lines: list[Line], method: str, sink: Sink) -> None:
    by_page: dict[int, list[Line]] = {}
    for ln in lines:
        by_page.setdefault(ln.page, []).append(ln)
    for page, plines in by_page.items():
        _page(page, plines, method, sink)
        if sink.full:
            return


def _page(page: int, lines: list[Line], method: str, sink: Sink) -> None:
    table_no = 0
    pipe_header: list[str] | None = None
    grid_header: Line | None = None
    row_no = 0
    for i, ln in enumerate(lines):
        if sink.full:
            return
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
        # 2. column-aligned tables: consecutive multi-cell lines without `label: value` cells
        gridish = len(ln.cells) >= 2 and not any(_is_kv(c.text) for c in ln.cells)
        if gridish and grid_header is not None and adjacent(lines[i - 1], ln):
            row_no += 1
            for c in ln.cells:
                col = _nearest(grid_header, c.x0, c.x1)
                h = grid_header.cells[col - 1].text
                loc = _loc(ln, table=table_no, row=row_no, col=col)
                sink.add(_rec(h, c.text, loc, f"p{page}t{table_no}r{row_no}", f"{method}-table", conf * 0.9))
            continue
        nxt = lines[i + 1] if i + 1 < len(lines) else None
        if (
            gridish
            and nxt is not None
            and adjacent(ln, nxt)
            and len(nxt.cells) >= 2
            and not any(_is_kv(c.text) for c in nxt.cells)
        ):
            grid_header, table_no, row_no = ln, table_no + 1, 0
            continue
        grid_header = None
        # 3. `label: value` cells (a line may hold several, e.g. title blocks)
        for cno, c in enumerate(ln.cells, start=1):
            m = LABEL_VALUE.match(c.text)
            if m:
                loc = _loc(ln, cell=cno) if len(ln.cells) > 1 else _loc(ln)
                sink.add(_rec(m.group(1), m.group(2).strip(), loc, "doc", f"{method}-text", conf))
