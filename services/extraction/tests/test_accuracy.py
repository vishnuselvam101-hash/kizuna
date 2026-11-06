"""Extraction accuracy on generated fixtures with known ground truth (every expected claim, exact
locations)."""

import base64
import json

from helpers import build_pdf, make_xlsx, text_at

from kizuna_extraction.ocr import parse_tsv
from kizuna_extraction.records import Sink, records_from_lines
from kizuna_extraction.worker import XLSX, handle


def run(media_type: str, data: bytes) -> dict:
    req = {"media_type": media_type, "data_b64": base64.b64encode(data).decode()}
    return handle(json.dumps(req).encode())


def drawing_pdf() -> bytes:
    p1 = [
        text_at(72, 740, "Part Number: BRK-100"),
        text_at(72, 726, "Material: S355"),
        text_at(72, 690, "Feature"),
        text_at(200, 690, "Length (mm)"),
        text_at(320, 690, "Tolerance (mm)"),
        text_at(72, 676, "A"),
        text_at(200, 676, "120.5"),
        text_at(320, 676, "±0.1"),
        text_at(72, 662, "B"),
        text_at(200, 662, "40"),
        text_at(320, 662, "±0.05"),
        # title block: label row over value row, plus a line with two `label: value` cells
        text_at(380, 120, "TITLE"),
        text_at(480, 120, "DWG NO."),
        text_at(380, 106, "BRACKET, MAIN"),
        text_at(480, 106, "D-1001"),
        text_at(380, 80, "Revision: C"),
        text_at(480, 80, "Scale: 1:2"),
    ]
    p2 = [text_at(72, 740, "| Lot | Hardness (HRC) |"), text_at(72, 726, "| L-7 | 58 |")]
    return build_pdf([" ".join(p1), " ".join(p2)], info={"Title": "Bracket main", "Author": "Eng"})


def claims_of(r: dict) -> set[tuple[str, str]]:
    return {(c["field"], c["value"]) for c in r["claims"]}


def test_pdf_drawing_claims_exact():
    r = run("application/pdf", drawing_pdf())
    assert r["ok"], r
    expected = {
        ("part_number", "BRK-100"),
        ("material", "S355"),
        ("dimension.length", "120.5"),
        ("dimension.length", "40"),
        ("tolerance", "±0.1"),
        ("tolerance", "±0.05"),
        ("title", "BRACKET, MAIN"),
        ("drawing_number", "D-1001"),
        ("revision", "C"),
        ("scale", "1:2"),
        ("lot", "L-7"),
        ("property.hardness", "58"),
        ("title", "Bracket main"),  # PDF document info
    }
    got = claims_of(r)
    assert got == expected, (got - expected, expected - got)
    assert r["unmapped_labels"] == ["author", "feature"]
    by = {(c["field"], c["value"]): c for c in r["claims"]}
    length = by[("dimension.length", "120.5")]
    assert length["unit"] == "mm" and length["number"] == "120.5"
    assert {k: length["location"][k] for k in ("page", "line", "table", "row", "col")} == {
        "page": 1,
        "line": 4,
        "table": 1,
        "row": 1,
        "col": 2,
    }
    assert length["method"] == "pdf-table+rules@2" and 0 < length["confidence"] < 1
    tol = by[("tolerance", "±0.05")]
    assert tol["number"] == "0.05" and tol["unit"] == "mm" and tol["symmetric"] is True
    assert by[("drawing_number", "D-1001")]["location"]["line"] == 7
    assert by[("scale", "1:2")]["location"]["cell"] == 2
    assert by[("property.hardness", "58")]["location"] == {
        "page": 2,
        "line": 2,
        "table": 1,
        "row": 1,
        "col": 2,
        "bbox": by[("property.hardness", "58")]["location"]["bbox"],
        "layout": "table",  # which layout reader produced it (calibration group, pdf@3)
    }
    assert by[("title", "Bracket main")]["location"] == {"metadata": "/Title"}


def test_xlsx_sheet_cell_and_cached_formula():
    ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    sheet = (
        f'<worksheet xmlns="{ns}"><sheetData>'
        '<row r="1"><c r="A1" t="inlineStr"><is><t>Lot</t></is></c>'
        '<c r="B1" t="inlineStr"><is><t>Measured Length (mm)</t></is></c>'
        '<c r="C1" t="inlineStr"><is><t>Result</t></is></c></row>'
        '<row r="3"><c r="A3" t="inlineStr"><is><t>L-9</t></is></c><c r="B3"><f>B2*2</f><v>99.8</v></c>'
        '<c r="C3" t="inlineStr"><is><t>OK</t></is></c></row>'
        "</sheetData></worksheet>"
    ).encode()
    r = run(XLSX, make_xlsx([["x"]], extra={"xl/worksheets/sheet1.xml": sheet}))
    assert r["ok"], r
    assert claims_of(r) == {("lot", "L-9"), ("measured.length", "99.8"), ("result", "OK")}
    m = next(c for c in r["claims"] if c["field"] == "measured.length")
    assert m["location"] == {"row": 3, "col": 2, "sheet": "Spec", "cell": "B3"}
    assert m["method"] == "xlsx-cell-cached-formula+rules@2" and m["unit"] == "mm"


def test_csv_japanese_cp932():
    r = run("text/csv", "品番,材質,数量\nBRK-1,SS400,12\n".encode("cp932"))
    assert r["ok"], r
    assert claims_of(r) == {("part_number", "BRK-1"), ("material", "SS400"), ("quantity", "12")}
    assert "csv_decoded_as:cp932" in r["warnings"]


DXF = "\n".join(
    [
        "0", "SECTION", "2", "HEADER",
        "9", "$ACADVER", "1", "AC1032",
        "9", "$INSUNITS", "70", "4",
        "9", "$CUSTOMPROPERTYTAG", "1", "Part Number",
        "9", "$CUSTOMPROPERTY", "1", "BRK-200",
        "0", "ENDSEC",
        "0", "SECTION", "2", "ENTITIES",
        "0", "ATTRIB", "5", "2F", "2", "DWG NO", "1", "D-2002",
        "0", "ATTRIB", "5", "30", "2", "REV", "1", "B",
        "0", "ENDSEC", "0", "EOF",
    ]
)  # fmt: skip


def test_dxf_header_and_title_block_attributes():
    r = run("image/vnd.dxf", DXF.encode())
    assert r["ok"], r
    assert claims_of(r) == {("part_number", "BRK-200"), ("drawing_number", "D-2002"), ("revision", "B")}
    labels = {x["label"]: x for x in r["records"]}
    assert labels["$ACADVER"]["value"] == "AC1032"
    assert labels["DWG NO"]["location"] == {"section": "ENTITIES", "line": 27, "handle": "2F"}


def test_ocr_tsv_lines_keep_pixel_boxes_and_confidence():
    hdr = "level\tpage_num\tblock_num\tpar_num\tline_num\tword_num\tleft\ttop\twidth\theight\tconf\ttext"
    rows = [
        "5\t1\t1\t1\t1\t1\t10\t20\t60\t18\t96\tPart",
        "5\t1\t1\t1\t1\t2\t75\t20\t90\t18\t90\tNumber:",
        "5\t1\t1\t1\t1\t3\t170\t20\t80\t18\t84\tBRK-1",
    ]
    lines = parse_tsv("\n".join([hdr, *rows]), page=1)
    sink = Sink(10)
    records_from_lines(lines, "ocr", sink)
    (rec,) = sink.items
    assert (rec["label"], rec["value"], rec["method"]) == ("Part Number", "BRK-1", "ocr-text")
    # a value cell carries its own lowest word confidence (84), not the line average
    assert rec["confidence"] == 0.84 and rec["location"]["bbox_px"] == [10, 20, 250, 38]
