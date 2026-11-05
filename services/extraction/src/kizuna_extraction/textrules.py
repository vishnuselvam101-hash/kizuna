"""Turns extracted text into records: `label: value` lines and pipe-delimited tables."""

import re

LABEL_VALUE = re.compile(r"^\s*([^\W\d_][\w .#/()\-]{0,40}?)\s*[:=：]\s*(\S.{0,200})$")


def records_from_text(pages: list[str], max_records: int) -> list[dict]:
    out: list[dict] = []
    for pno, text in enumerate(pages, start=1):
        header: list[str] | None = None
        row_no = 0
        for lno, raw in enumerate(text.splitlines(), start=1):
            line = raw.strip()
            if len(out) >= max_records:
                return out
            if line.startswith("|") and line.endswith("|") and line.count("|") >= 3:
                cells = [c.strip() for c in line.strip("|").split("|")]
                if all(set(c) <= set("-: ") for c in cells):
                    continue  # markdown separator row
                if header is None:
                    header = cells
                    continue
                row_no += 1
                for col, (h, v) in enumerate(zip(header, cells, strict=False), start=1):
                    if h and v:
                        loc = {"page": pno, "line": lno, "row": row_no, "col": col}
                        out.append({"label": h, "value": v, "location": loc, "group": f"p{pno}r{row_no}"})
                continue
            header = None
            m = LABEL_VALUE.match(line)
            if m:
                loc = {"page": pno, "line": lno}
                out.append(
                    {"label": m.group(1), "value": m.group(2).strip(), "location": loc, "group": "doc"}
                )
    return out
