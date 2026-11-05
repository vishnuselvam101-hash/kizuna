import io
import zipfile


def make_pdf(lines: list[str]) -> bytes:
    def esc(s: str) -> str:
        return s.replace("\\", "\\\\").replace("(", "\\(").replace(")", "\\)")

    content = "BT /F1 11 Tf 72 740 Td 14 TL " + " ".join(f"({esc(t)}) '" for t in lines) + " ET"
    objs = [
        "<< /Type /Catalog /Pages 2 0 R >>",
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>",
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 612 792] /Contents 4 0 R "
        "/Resources << /Font << /F1 5 0 R >> >> >>",
        f"<< /Length {len(content)} >>\nstream\n{content}\nendstream",
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>",
    ]
    out = b"%PDF-1.4\n"
    offsets = []
    for i, o in enumerate(objs, start=1):
        offsets.append(len(out))
        out += f"{i} 0 obj\n{o}\nendobj\n".encode()
    xref = len(out)
    out += f"xref\n0 {len(objs) + 1}\n0000000000 65535 f \n".encode()
    for off in offsets:
        out += f"{off:010d} 00000 n \n".encode()
    out += f"trailer\n<< /Size {len(objs) + 1} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n".encode()
    return out


def make_xlsx(rows: list[list[str]], extra: dict[str, bytes] | None = None, shared: bool = True) -> bytes:
    ns = "http://schemas.openxmlformats.org/spreadsheetml/2006/main"
    strings: list[str] = []
    xml_rows = []
    for r, row in enumerate(rows, start=1):
        cells = []
        for c, v in enumerate(row):
            ref = f"{chr(65 + c)}{r}"
            if shared:
                strings.append(v)
                cells.append(f'<c r="{ref}" t="s"><v>{len(strings) - 1}</v></c>')
            else:
                cells.append(f'<c r="{ref}" t="inlineStr"><is><t>{v}</t></is></c>')
        xml_rows.append(f'<row r="{r}">{"".join(cells)}</row>')
    sheet = f'<worksheet xmlns="{ns}"><sheetData>{"".join(xml_rows)}</sheetData></worksheet>'
    sst = f'<sst xmlns="{ns}">' + "".join(f"<si><t>{s}</t></si>" for s in strings) + "</sst>"
    rel = "http://schemas.openxmlformats.org/officeDocument/2006/relationships"
    files = {
        "[Content_Types].xml": b"<Types/>",
        "xl/workbook.xml": (
            f'<workbook xmlns="{ns}" xmlns:r="{rel}"><sheets>'
            '<sheet name="Spec" sheetId="1" r:id="rId1"/></sheets></workbook>'
        ).encode(),
        "xl/_rels/workbook.xml.rels": (
            b'<Relationships><Relationship Id="rId1" Target="worksheets/sheet1.xml"/></Relationships>'
        ),
        "xl/worksheets/sheet1.xml": sheet.encode(),
        "xl/sharedStrings.xml": sst.encode(),
    }
    files.update(extra or {})
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for k, v in files.items():
            z.writestr(k, v)
    return buf.getvalue()
