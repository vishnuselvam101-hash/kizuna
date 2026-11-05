"""Drawing metadata: title-block fields from PDF text and document info, and ASCII DXF header/attributes.

DXF is parsed with a small stdlib reader (group-code/value pairs) instead of ezdxf: only the HEADER section
and
ATTRIB/ATTDEF entities are needed, and ezdxf would add numpy/fonttools to the sandbox
(docs/research/dependencies.md).
"""

import re

from .errors import ExtractionError
from .layout import Line
from .records import LABEL_VALUE, Sink, _loc, _rec

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
}


def _norm(s: str) -> str:
    return re.sub(r"\s+", " ", s.strip().lower())


def title_block(lines: list[Line], sink: Sink) -> None:
    """Label cell followed by a value cell on the same line, or by the cell directly below it."""
    seen = {(r["location"].get("page"), r["location"].get("line"), r["value"]) for r in sink.items}
    by_page: dict[int, list[Line]] = {}
    for ln in lines:
        by_page.setdefault(ln.page, []).append(ln)
    for plines in by_page.values():
        for i, ln in enumerate(plines):
            for ci, c in enumerate(ln.cells):
                if _norm(c.text) not in TITLE_LABELS:
                    continue
                label = c.text.strip().rstrip(".")
                value_cell, at = None, ln
                if ci + 1 < len(ln.cells) and _norm(ln.cells[ci + 1].text) not in TITLE_LABELS:
                    value_cell = ln.cells[ci + 1]
                    loc = _loc(ln, cell=ci + 2)
                elif i + 1 < len(plines):
                    below = plines[i + 1]
                    tol = max(4.0, (c.x1 - c.x0) * 0.5)
                    for bj, b in enumerate(below.cells, start=1):
                        if abs(b.x0 - c.x0) <= tol and _norm(b.text) not in TITLE_LABELS:
                            value_cell, at = b, below
                            loc = _loc(below, cell=bj)
                            break
                if value_cell is None or LABEL_VALUE.match(value_cell.text):
                    continue
                if (at.page, at.no, value_cell.text) in seen:
                    continue  # already extracted as a table cell
                sink.add(
                    _rec(label, value_cell.text, loc, "title_block", "pdf-title-block", 0.75 * at.confidence)
                )


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
