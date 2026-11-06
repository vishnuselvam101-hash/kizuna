"""Drawing metadata: title-block fields from PDF text and document info, and ASCII DXF header/attributes.

DXF is parsed with a small stdlib reader (group-code/value pairs) instead of ezdxf: only the HEADER section
and
ATTRIB/ATTDEF entities are needed, and ezdxf would add numpy/fonttools to the sandbox
(docs/research/dependencies.md).
"""

import re

from .claims import label_like, lookup, normalize_label
from .errors import ExtractionError
from .ja import parse_date
from .layout import Line
from .records import LABEL_VALUE, Sink, _loc, _rec

# Approval boxes (印影 / sign-off): role label above a stamp or name, often a date below that.
STAMP_LABELS = {"設計", "作図", "製図", "検図", "照査", "承認"}

# Title-block labels that often stand alone in one cell with the value to the right or directly below.
TITLE_LABELS = {
    "title",
    "drawing title",
    "dwg no",
    "dwg no.",
    "drawing no",
    "drawing no.",
    "drawing number",
    "part no",
    "part no.",
    "part number",
    "rev",
    "rev.",
    "revision",
    "scale",
    "sheet",
    "size",
    "material",
    "drawn",
    "drawn by",
    "checked",
    "checked by",
    "approved",
    "approved by",
    "date",
    "図番",
    "品番",
    "材質",
    "尺度",
    "改訂",
    *STAMP_LABELS,
}


def _norm(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip().lower())


def _region(top: Line, bottom: Line, x0: float, x1: float) -> list[int]:
    """The approval box from the role label down to the stamp/name, as whole OCR pixels or PDF points
    ([x0, y0, x1, y1]; PDF y grows upwards)."""
    if "px_bbox" in top.extra and "px_bbox" in bottom.extra:
        return [int(x0), top.extra["px_bbox"][1], int(x1), bottom.extra["px_bbox"][3]]
    if top.y is not None and bottom.y is not None:
        y0, y1 = bottom.y - bottom.size * 0.3, top.y + top.size
        return [max(0, round(v)) for v in (x0, y0, x1, y1)]
    return []


def _stamp_date(plines: list[Line], start: int, c, label: str, sink: Sink, method: str) -> None:
    """A date written under the stamp (up to two lines further down, same column) belongs to that sign-off."""
    tol = max(8.0, (c.x1 - c.x0))
    for ln in plines[start : start + 2]:
        for bj, b in enumerate(ln.cells, start=1):
            if abs(b.x0 - c.x0) <= tol and parse_date(b.text) is not None:
                conf = 0.75 * (b.conf if b.conf is not None else ln.confidence)
                sink.add(_rec(label, b.text, _loc(ln, cell=bj), "title_block", f"{method}-title-block", conf))
                return


def _title_label(text: str) -> bool:
    """A cell that is only a label: a title-block label, or (v3) any label in the vocabulary."""
    t = text.strip()
    if _norm(t) in TITLE_LABELS:
        return True
    return len(t) <= 24 and not any(ch in t for ch in ":：") and lookup(normalize_label(t)[0]) is not None


def _value_cell(c) -> bool:
    return not LABEL_VALUE.match(c.text) and not label_like(c.text) and not _title_label(c.text)


def title_block(lines: list[Line], sink: Sink, method: str = "pdf") -> None:
    """Label cell followed by a value cell on the same line, or by the cell directly below it, or (v3) with
    the
    value directly above it (title blocks with the caption under the value). Values already read by another
    layout reader are not read again."""
    seen = {(r["location"].get("page"), r["location"].get("line"), r["value"]) for r in sink.items}
    read_lines = {(r["location"].get("page"), r["location"].get("line")) for r in sink.items}
    by_page: dict[int, list[Line]] = {}
    for ln in lines:
        by_page.setdefault(ln.page, []).append(ln)
    for plines in by_page.values():
        for i, ln in enumerate(plines):
            for ci, c in enumerate(ln.cells):
                if (
                    not _title_label(c.text)
                    or ln.extra.get("read")
                    or (ln.page, ln.no) in read_lines
                    and c.text != ln.text
                ):
                    continue  # table headers, row labels and strips are not title-block labels
                label = c.text.strip().rstrip(".")
                value_cell, at, loc = None, ln, {}
                tol = max(4.0, (c.x1 - c.x0) * 0.5)
                cands = []
                if ci + 1 < len(ln.cells):
                    cands.append((ln, ci + 1, ln.cells[ci + 1], len(ln.cells) > 1))
                below = plines[i + 1] if i + 1 < len(plines) else None
                above = plines[i - 1] if i and _stacked(ln, plines[i - 1]) else None
                for other in (below, above):
                    if other is None:
                        continue
                    for bj, b in enumerate(other.cells):
                        if abs(b.x0 - c.x0) <= tol or (min(b.x1, c.x1) - max(b.x0, c.x0)) > 0.5 * (
                            c.x1 - c.x0
                        ):
                            cands.append((other, bj, b, True))
                            break
                for other, bj, b, cell_loc in cands:
                    if (
                        not _value_cell(b)
                        or (other.page, other.no, b.text) in seen
                        or other.extra.get("read")
                    ):
                        continue
                    value_cell, at = b, other
                    loc = _loc(other, cell=bj + 1) if cell_loc else _loc(other)
                    break
                if value_cell is None:
                    continue
                seen.add((at.page, at.no, value_cell.text))
                conf = 0.75 * (value_cell.conf if value_cell.conf is not None else at.confidence)
                if label in STAMP_LABELS and at is not ln:
                    top, bottom = (ln, at) if at.no > ln.no else (at, ln)
                    loc["region"] = _region(top, bottom, c.x0, max(c.x1, value_cell.x1))
                    if at.no > ln.no:
                        _stamp_date(plines, i + 2, c, label, sink, method)
                sink.add(_rec(label, value_cell.text, loc, "title_block", f"{method}-title-block", conf))


def _stacked(a: Line, b: Line) -> bool:
    """`b` is the line directly above or below `a` (next/previous line number, at most ~2.6 line heights)."""
    if a.page != b.page or abs(a.no - b.no) != 1:
        return False
    if a.y is None or b.y is None:
        return True
    return abs(a.y - b.y) <= 2.6 * max(a.size, b.size, 1.0)


def pdf_metadata(meta: dict[str, str], sink: Sink) -> None:
    for key, value in meta.items():
        sink.add(_rec(key.lstrip("/"), value, {"metadata": key}, "pdf_info", "pdf-metadata", 0.7))


DXF_HEADER_VARS = {
    "$ACADVER",
    "$INSUNITS",
    "$MEASUREMENT",
    "$TDCREATE",
    "$TDUPDATE",
    "$LASTSAVEDBY",
    "$PROJECTNAME",
}


def dxf_records(data: bytes, limits: dict, sink: Sink) -> list[str]:
    if data.startswith(b"AutoCAD Binary DXF"):
        raise ExtractionError("dxf_binary_unsupported")
    try:
        text = data.decode("utf-8")
    except UnicodeDecodeError:
        text = data.decode("cp1252", "replace")
    raw = text.splitlines()
    if len(raw) % 2:
        raw.append("")
    if len(raw) // 2 > limits["max_dxf_pairs"]:
        raise ExtractionError("dxf_too_large")
    section = None
    var: str | None = None
    entity: dict | None = None
    prop_tag: str | None = None
    warnings: list[str] = []

    def flush(ent: dict | None) -> None:
        if ent and ent["type"] in ("ATTRIB", "ATTDEF") and ent.get("tag") and ent.get("value"):
            loc = {"section": "ENTITIES", "line": ent["line"]}
            if ent.get("handle"):
                loc["handle"] = ent["handle"][:32]
            sink.add(_rec(ent["tag"], ent["value"], loc, "title_block", "dxf-attrib", 0.9))

    for i in range(0, len(raw), 2):
        code_s, value = raw[i].strip(), raw[i + 1].strip()
        line_no = i + 1
        try:
            code = int(code_s)
        except ValueError as e:
            raise ExtractionError("dxf_malformed") from e
        if code == 0:
            flush(entity)
            entity = None
            if value == "SECTION":
                section = "?"
            elif value == "ENDSEC":
                section = None
            elif value == "EOF":
                break
            elif section in ("ENTITIES", "BLOCKS"):
                entity = {"type": value, "line": line_no}
            continue
        if code == 2 and section == "?":
            section = value
            continue
        if section == "HEADER":
            if code == 9:
                var = value
                continue
            if var == "$CUSTOMPROPERTYTAG" and code == 1:
                prop_tag = value
            elif var == "$CUSTOMPROPERTY" and code == 1 and prop_tag:
                loc = {"section": "HEADER", "variable": "$CUSTOMPROPERTY", "line": line_no}
                sink.add(_rec(prop_tag, value, loc, "dxf_header", "dxf-header", 0.95))
                prop_tag = None
            elif var in DXF_HEADER_VARS:
                loc = {"section": "HEADER", "variable": var, "line": line_no}
                sink.add(_rec(var, value, loc, "dxf_header", "dxf-header", 0.95))
                var = None
        elif entity is not None:
            if code == 2:
                entity["tag"] = value
            elif code == 1:
                entity["value"] = value
            elif code == 5:
                entity["handle"] = value
        if sink.full:
            warnings.append("records_truncated")
            break
    flush(entity)
    return warnings
