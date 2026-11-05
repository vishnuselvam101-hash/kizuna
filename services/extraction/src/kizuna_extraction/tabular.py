"""CSV and XLSX extraction. XLSX is read with zipfile + ElementTree under explicit size limits;
formulas are never evaluated (only cached values are read) and DTDs/entities are refused."""

import csv
import io
import re
import xml.etree.ElementTree as ET
import zipfile

NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
REL_NS = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"


class ExtractionError(Exception):
    pass


def rows_to_records(rows, max_records: int, sheet: str | None = None) -> list[dict]:
    out: list[dict] = []
    header: list[str] | None = None
    for rno, row in enumerate(rows, start=1):
        cells = [(c or "").strip() for c in row]
        if header is None:
            if any(cells):
                header = cells
            continue
        for cno, (h, v) in enumerate(zip(header, cells, strict=False), start=1):
            if not h or not v:
                continue
            if len(out) >= max_records:
                return out
            loc: dict = {"row": rno, "col": cno}
            if sheet is not None:
                loc["sheet"] = sheet[:120]
                loc["cell"] = f"{col_name(cno)}{rno}"
            out.append({"label": h, "value": v, "location": loc, "group": f"{sheet or 'csv'}:{rno}"})
    return out


def col_name(n: int) -> str:
    s = ""
    while n:
        n, r = divmod(n - 1, 26)
        s = chr(65 + r) + s
    return s


def csv_records(data: bytes, limits: dict) -> list[dict]:
    text = data.decode("utf-8-sig")
    sample = text[:4096]
    try:
        dialect = csv.Sniffer().sniff(sample, delimiters=",;\t")
    except csv.Error:
        dialect = csv.excel
    reader = csv.reader(io.StringIO(text), dialect)
    return rows_to_records(reader, limits["max_records"])


def _read_member(z: zipfile.ZipFile, name: str, limit: int) -> bytes:
    info = z.getinfo(name)
    if info.file_size > limit:
        raise ExtractionError("xlsx_member_too_large")
    with z.open(info) as f:
        data = f.read(limit + 1)
    if len(data) > limit:
        raise ExtractionError("xlsx_member_too_large")
    if b"<!DOCTYPE" in data[:4096] or b"<!ENTITY" in data:
        raise ExtractionError("xlsx_dtd_refused")
    return data


def _parse(xml: bytes) -> ET.Element:
    # Safe here: _read_member refuses any DTD/ENTITY declaration before parsing, ElementTree never
    # resolves external entities, and expat >= 2.4 bounds entity amplification. Sizes are capped.
    return ET.fromstring(xml)  # noqa: S314


def _ref_to_col(ref: str) -> int:
    m = re.match(r"([A-Z]+)", ref or "")
    n = 0
    for ch in m.group(1) if m else "":
        n = n * 26 + ord(ch) - 64
    return n


def xlsx_records(data: bytes, limits: dict) -> list[dict]:
    member_limit = limits["max_member_bytes"]
    try:
        z = zipfile.ZipFile(io.BytesIO(data))
    except zipfile.BadZipFile as e:
        raise ExtractionError("xlsx_bad_zip") from e
    names = set(z.namelist())
    shared: list[str] = []
    if "xl/sharedStrings.xml" in names:
        root = _parse(_read_member(z, "xl/sharedStrings.xml", member_limit))
        for si in root.findall("m:si", NS):
            shared.append("".join(t.text or "" for t in si.iter(f"{{{NS['m']}}}t")))
    wb = _parse(_read_member(z, "xl/workbook.xml", member_limit))
    rels = {}
    if "xl/_rels/workbook.xml.rels" in names:
        rroot = _parse(_read_member(z, "xl/_rels/workbook.xml.rels", member_limit))
        for r in rroot:
            rels[r.get("Id")] = r.get("Target", "")
    out: list[dict] = []
    for sh in wb.findall("m:sheets/m:sheet", NS)[: limits["max_sheets"]]:
        target = rels.get(sh.get(REL_NS), "")
        path = "xl/" + target.lstrip("/").removeprefix("xl/")
        if path not in names or ".." in path:
            continue
        root = _parse(_read_member(z, path, member_limit))
        rows = []
        for row in root.findall("m:sheetData/m:row", NS):
            cells: dict[int, str] = {}
            for c in row.findall("m:c", NS):
                t = c.get("t")
                if t == "inlineStr":
                    val = "".join(x.text or "" for x in c.iter(f"{{{NS['m']}}}t"))
                else:
                    v = c.find("m:v", NS)  # cached value only; formulas are never evaluated
                    val = v.text if v is not None and v.text else ""
                    if t == "s" and val.isdigit() and int(val) < len(shared):
                        val = shared[int(val)]
                cells[_ref_to_col(c.get("r", ""))] = val
            width = max(cells) if cells else 0
            rows.append([cells.get(i, "") for i in range(1, width + 1)])
        out.extend(rows_to_records(rows, limits["max_records"] - len(out), sheet=sh.get("name", "sheet")))
    return out
