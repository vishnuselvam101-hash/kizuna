"""CSV and XLSX extraction. XLSX is read with zipfile + ElementTree under explicit limits (member count,
per-member
and total inflated bytes, compression ratio, cell count); formulas are never evaluated (only cached values
are read)
and DTDs/entities are refused. Paths inside the archive are only ever looked up in the archive itself."""

import csv
import io
import re
import xml.etree.ElementTree as ET
import zipfile
from posixpath import normpath
from xml.parsers import expat

from .claims import lookup, normalize_label
from .errors import ExtractionError
from .records import Sink, _rec

NS = {"m": "http://schemas.openxmlformats.org/spreadsheetml/2006/main"}
REL_NS = "{http://schemas.openxmlformats.org/officeDocument/2006/relationships}id"
MAX_RATIO = 200  # inflated/compressed; real sheets are ~5-30x


def col_name(n: int) -> str:
    s = ""
    while n:
        n, r = divmod(n - 1, 26)
        s = chr(65 + r) + s
    return s


def rows_to_records(
    rows, sink: Sink, sheet: str | None, method: str, formulas: set | None = None, header_row: int = 0
) -> None:
    """`header_row` N: row N is the header, earlier rows skipped. 0: detected header, the first row with
    at least three cells most of which are known labels (within the first PREAMBLE_ROWS rows; Japanese exports
    put a title, issue date and addressee above the table), else the first non-empty row. Two-cell
    `label,value` rows above a detected header become document-level records."""
    header: list[str] | None = None
    pending: list[tuple[int, list[str]]] = []
    for rno, row in rows:
        cells = [(c or "").strip() for c in row]
        if header is None:
            if header_row:
                if rno >= header_row:
                    header = cells
                continue
            if not any(cells):
                continue
            pending.append((rno, cells))
            if _header_like(cells):
                header = cells
                _preamble(pending[:-1], sink, method)
                pending = []
            elif len(pending) >= PREAMBLE_ROWS:
                (_r, header), rest = pending[0], pending[1:]
                pending = []
                _body(rest, header, sink, sheet, method, formulas)
            continue
        _body([(rno, cells)], header, sink, sheet, method, formulas)
        if sink.full:
            return
    if header is None and pending:  # short file without a recognisable header: old behaviour
        (_r, header), rest = pending[0], pending[1:]
        _body(rest, header, sink, sheet, method, formulas)


PREAMBLE_ROWS = 20


def _header_like(cells: list[str]) -> bool:
    filled = [c for c in cells if c]
    known = sum(lookup(normalize_label(c)[0]) is not None for c in filled)
    return len(filled) >= 3 and known * 2 > len(filled)


def _preamble(rows: list[tuple[int, list[str]]], sink: Sink, method: str) -> None:
    for rno, cells in rows:
        filled = [(i, c) for i, c in enumerate(cells, start=1) if c]
        if len(filled) == 2 and lookup(normalize_label(filled[0][1])[0]) is not None:
            loc = {"row": rno, "col": filled[1][0]}
            sink.add(_rec(filled[0][1], filled[1][1], loc, "doc", method, 0.95))


def _body(rows, header: list[str], sink: Sink, sheet: str | None, method: str, formulas: set | None) -> None:
    for rno, cells in rows:
        for cno, (h, v) in enumerate(zip(header, cells, strict=False), start=1):
            if not h or not v:
                continue
            if sink.full:
                return
            loc: dict = {"row": rno, "col": cno}
            conf = 0.97
            m = method
            if sheet is not None:
                loc["sheet"] = sheet[:120]
                loc["cell"] = f"{col_name(cno)}{rno}"
                if formulas and loc["cell"] in formulas:
                    m, conf = f"{method}-cached-formula", 0.9
            sink.add(_rec(h, v, loc, f"{sheet or 'csv'}:{rno}", m, conf))


def csv_records(data: bytes, limits: dict, sink: Sink, opts: dict | None = None) -> list[str]:
    opts = opts or {}
    warnings: list[str] = []
    # An explicit encoding setting is binding: utf-8 never silently falls back to CP932 (bughunt).
    encodings = {"shift_jis": ("cp932",), "utf-8": ("utf-8-sig",)}.get(
        opts.get("csv_encoding") or "", ("utf-8-sig", "cp932")
    )
    if data.startswith((b"\xff\xfe", b"\xfe\xff")):
        encodings = ("utf-16",)  # BOM-marked UTF-16 (Excel "Unicode Text"); cp932 would garble every label
    for enc in encodings:
        try:
            text = data.decode(enc)
        except UnicodeDecodeError:
            continue
        if "\x00" not in text:  # NULs mean a wrong guess (e.g. BOM-less UTF-16), never real CSV text
            break
    else:
        raise ExtractionError("csv_bad_encoding")
    if enc != "utf-8-sig":
        warnings.append(f"csv_decoded_as:{enc}")
    dialect: type[csv.Dialect] | csv.Dialect
    if opts.get("csv_delimiter"):

        class _Fixed(csv.excel):
            delimiter = opts["csv_delimiter"]

        dialect = _Fixed
    else:
        try:
            dialect = csv.Sniffer().sniff(text[:4096], delimiters=",;\t")
        except csv.Error:
            dialect = csv.excel

    def rows():
        reader = csv.reader(io.StringIO(text), dialect)
        for rno, row in enumerate(reader, start=1):
            if rno > limits["max_rows"]:
                warnings.append("csv_rows_truncated")
                return
            yield rno, row[: limits["max_cols"]]

    # Cells up to max_cell_chars (default 1 MB; Python's default is 128 KB); larger: a clear error.
    csv.field_size_limit(limits["max_cell_chars"])
    try:
        rows_to_records(rows(), sink, None, "csv-cell", header_row=opts.get("header_row", 0))
    except csv.Error as e:
        if "field larger than field limit" in str(e):
            raise ExtractionError("csv_cell_too_large") from e
        raise ExtractionError("csv_malformed") from e
    return warnings


class _Zip:
    def __init__(self, data: bytes, limits: dict):
        try:
            self.z = zipfile.ZipFile(io.BytesIO(data))
            infos = self.z.infolist()
        except (zipfile.BadZipFile, ValueError, EOFError) as e:
            raise ExtractionError("xlsx_bad_zip") from e
        if len(infos) > limits["max_zip_members"]:
            raise ExtractionError("xlsx_too_many_members")
        declared = sum(i.file_size for i in infos)
        if declared > limits["max_total_unzipped"]:
            raise ExtractionError("xlsx_zip_bomb")
        for i in infos:
            if i.file_size > 1024 * 1024 and i.file_size > MAX_RATIO * max(i.compress_size, 1):
                raise ExtractionError("xlsx_zip_bomb")
        self.names = {i.filename for i in infos}
        self.limits = limits
        self.read_total = 0

    def read(self, name: str) -> bytes:
        limit = self.limits["max_member_bytes"]
        info = self.z.getinfo(name)
        if info.file_size > limit:
            raise ExtractionError("xlsx_member_too_large")
        try:
            with self.z.open(info) as f:
                data = f.read(limit + 1)  # header sizes can lie: cap the actual inflated bytes too
        except (zipfile.BadZipFile, EOFError, NotImplementedError, RuntimeError) as e:
            raise ExtractionError("xlsx_bad_zip") from e
        if len(data) > limit:
            raise ExtractionError("xlsx_member_too_large")
        self.read_total += len(data)
        if self.read_total > self.limits["max_total_unzipped"]:
            raise ExtractionError("xlsx_zip_bomb")
        _refuse_dtd(data)
        return data

    def xml(self, name: str) -> ET.Element:
        try:
            # Safe here: read() refuses any DTD/ENTITY declaration before parsing, ElementTree never resolves
            # external entities, and expat >= 2.4 bounds entity amplification. Sizes are capped.
            return ET.fromstring(self.read(name))  # noqa: S314
        except ET.ParseError as e:
            raise ExtractionError("xlsx_bad_xml") from e


def _refuse_dtd(data: bytes) -> None:
    """Refuses DOCTYPE/ENTITY at the parser level, in any encoding expat reads (review finding L4)."""
    p = expat.ParserCreate()

    def refuse(*_a: object) -> None:
        raise ExtractionError("xlsx_dtd_refused")

    p.StartDoctypeDeclHandler = refuse
    p.EntityDeclHandler = refuse
    try:
        p.Parse(data, True)
    except expat.ExpatError as e:
        raise ExtractionError("xlsx_bad_xml") from e


def _ref_to_col(ref: str) -> int:
    m = re.match(r"([A-Z]{1,3})", ref or "")
    n = 0
    for ch in m.group(1) if m else "":
        n = n * 26 + ord(ch) - 64
    return n


def _sheet_path(target: str) -> str | None:
    p = normpath("xl/" + target.lstrip("/").removeprefix("xl/"))
    return p if p.startswith("xl/") else None


def xlsx_records(data: bytes, limits: dict, sink: Sink, opts: dict | None = None) -> list[str]:
    opts = opts or {}
    z = _Zip(data, limits)
    warnings: list[str] = []
    shared: list[str] = []
    if "xl/sharedStrings.xml" in z.names:
        for si in z.xml("xl/sharedStrings.xml").findall("m:si", NS):
            shared.append("".join(t.text or "" for t in si.iter(f"{{{NS['m']}}}t")))
    if "xl/workbook.xml" not in z.names:
        raise ExtractionError("xlsx_no_workbook")
    wb = z.xml("xl/workbook.xml")
    rels = {}
    if "xl/_rels/workbook.xml.rels" in z.names:
        for r in z.xml("xl/_rels/workbook.xml.rels"):
            rels[r.get("Id")] = r.get("Target", "")
    sheets = wb.findall("m:sheets/m:sheet", NS)
    if opts.get("xlsx_sheet"):
        sheets = [sh for sh in sheets if sh.get("name") == opts["xlsx_sheet"]]
        if not sheets:
            warnings.append("xlsx_sheet_not_found")
    if len(sheets) > limits["max_sheets"]:
        warnings.append("xlsx_sheets_truncated")
    cells_seen = 0
    for sh in sheets[: limits["max_sheets"]]:
        path = _sheet_path(rels.get(sh.get(REL_NS), ""))
        if path is None or path not in z.names:
            warnings.append("xlsx_sheet_target_invalid")
            continue
        root = z.xml(path)
        rows = []
        formulas: set[str] = set()
        for row in root.findall("m:sheetData/m:row", NS):
            cells: dict[int, str] = {}
            for c in row.findall("m:c", NS):
                cells_seen += 1
                if cells_seen > limits["max_cells"]:
                    raise ExtractionError("xlsx_too_many_cells")
                col = _ref_to_col(c.get("r", ""))
                if not 0 < col <= limits["max_cols"]:
                    continue
                t = c.get("t")
                if t == "inlineStr":
                    val = "".join(x.text or "" for x in c.iter(f"{{{NS['m']}}}t"))
                else:
                    v = c.find("m:v", NS)  # cached value only; formulas are never evaluated
                    val = v.text if v is not None and v.text else ""
                    if t == "s" and val.isdigit() and int(val) < len(shared):
                        val = shared[int(val)]
                if c.find("m:f", NS) is not None:
                    formulas.add(c.get("r", ""))
                cells[col] = val
            try:
                rno = int(row.get("r") or len(rows) + 1)
            except ValueError:
                rno = len(rows) + 1
            width = max(cells) if cells else 0
            rows.append((rno, [cells.get(i, "") for i in range(1, width + 1)]))
        rows_to_records(
            rows, sink, sh.get("name", "sheet"), "xlsx-cell", formulas, header_row=opts.get("header_row", 0)
        )
    return warnings
