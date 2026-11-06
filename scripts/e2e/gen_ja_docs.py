#!/usr/bin/env python3
"""Realistic Japanese manufacturing documents for the browser acceptance run (stdlib only, synthetic data).

Usage: gen_ja_docs.py OUT_DIR

Writes:
  発注一覧_SJIS.csv     purchase orders, Shift_JIS (cp932), one line on an old revision (wrong_revision)
  仕様書_JA-BRK-1001.xlsx  spec sheet (XLSX, inline strings) — material SUS304, rev B
  図面_JA-BRK-1001.dxf   ASCII DXF drawing with a Japanese title block (ATTRIB entities)
The PDFs (検査成績書, ミルシート, scanned drawing) are rendered by web/e2e/global-setup.ts with Chromium.
"""

import sys
import zipfile
from pathlib import Path
from xml.sax.saxutils import escape

PART = "JA-BRK-1001"


def csv_sjis() -> bytes:
    rows = [
        "注文番号,品番,改訂,数量,仕入先",
        f"PO-J-7001,{PART},B,120,東邦精工株式会社",
        f"PO-J-7002,{PART},A,40,東邦精工株式会社",
        "PO-J-7003,JA-SFT-2002,C,300,北陸金属工業",
        "PO-J-7004,JA-SFT2002,C,60,北陸金属工業",  # same part, identifier typed without a separator
    ]
    return ("\r\n".join(rows) + "\r\n").encode("cp932")


def xlsx() -> bytes:
    header = ["品番", "改訂", "材質", "長さ (mm)", "承認", "有効期限", "製造元"]
    rows = [header, [PART, "B", "SUS304", "120", "Released", "2031-03-31", "東邦精工株式会社"],
            ["JA-SFT-2002", "C", "S45C", "250", "Released", "2031-06-30", "北陸金属工業"]]

    def cell(r: int, c: int, v: str) -> str:
        ref = f"{chr(65 + c)}{r}"
        return f'<c r="{ref}" t="inlineStr"><is><t>{escape(v)}</t></is></c>'

    sheet_rows = "".join(
        f'<row r="{i + 1}">' + "".join(cell(i + 1, j, v) for j, v in enumerate(row)) + "</row>" for i, row in enumerate(rows)
    )
    files = {
        "[Content_Types].xml": '<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types">'
        '<Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/>'
        '<Default Extension="xml" ContentType="application/xml"/>'
        '<Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/>'
        '<Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/>'
        "</Types>",
        "_rels/.rels": '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/>'
        "</Relationships>",
        "xl/workbook.xml": '<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" '
        'xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="仕様" sheetId="1" r:id="rId1"/></sheets></workbook>',
        "xl/_rels/workbook.xml.rels": '<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships">'
        '<Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/>'
        "</Relationships>",
        "xl/worksheets/sheet1.xml": '<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main">'
        f"<sheetData>{sheet_rows}</sheetData></worksheet>",
    }
    import io

    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for name, text in files.items():
            z.writestr(name, text.encode("utf-8"))
    return buf.getvalue()


def dxf() -> bytes:
    pairs = [
        ("0", "SECTION"), ("2", "HEADER"),
        ("9", "$ACADVER"), ("1", "AC1027"),
        ("9", "$INSUNITS"), ("70", "4"),
        ("9", "$PROJECTNAME"), ("1", "ブラケット量産"),
        ("0", "ENDSEC"),
        ("0", "SECTION"), ("2", "ENTITIES"),
    ]
    attribs = [("図番", "D-JA-5001"), ("品番", PART), ("改訂", "B"), ("材質", "SUS304"), ("尺度", "1:2")]
    for i, (tag, val) in enumerate(attribs):
        pairs += [("0", "ATTRIB"), ("5", f"A{i + 1}"), ("8", "TITLE"), ("2", tag), ("1", val)]
    pairs += [("0", "ENDSEC"), ("0", "EOF")]
    return ("\n".join(f"{c}\n{v}" for c, v in pairs) + "\n").encode("utf-8")


def main() -> None:
    out = Path(sys.argv[1])
    out.mkdir(parents=True, exist_ok=True)
    (out / "発注一覧_SJIS.csv").write_bytes(csv_sjis())
    (out / f"仕様書_{PART}.xlsx").write_bytes(xlsx())
    (out / f"図面_{PART}.dxf").write_bytes(dxf())


if __name__ == "__main__":
    main()
