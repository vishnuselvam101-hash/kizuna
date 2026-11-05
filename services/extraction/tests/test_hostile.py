"""Malformed and hostile documents: every input gets a structured answer within limits, never a crash."""

import base64
import io
import json
import os
import struct
import time
import zipfile
import zlib

import pytest
from helpers import build_pdf, make_pdf, make_xlsx, text_at
from hypothesis import HealthCheck, given, settings
from hypothesis import strategies as st

from kizuna_extraction.worker import XLSX, handle


def run(media_type: str, data: bytes, **limits) -> dict:
    req = {"media_type": media_type, "data_b64": base64.b64encode(data).decode(), "limits": limits}
    return handle(json.dumps(req).encode())


# --- PDF ---------------------------------------------------------------------------------------------------


@pytest.mark.parametrize(
    "blob",
    [
        b"",
        b"%PDF-1.7\n",
        b"%PDF-1.4\n1 0 obj << /Length 999999 >>\nstream\nxx",
        b"\x00" * 100,
        b"%PDF-1.4\n1 0 obj << /Type /Catalog /Pages 1 0 R >> endobj trailer << /Root 1 0 R >>",  # self-loop
    ],
)
def test_malformed_pdf_returns_structured_answer(blob):
    r = run("application/pdf", blob)
    assert r["ok"] is False or r["records"] == []


@pytest.mark.parametrize(
    "blob",
    [
        make_pdf(["Part Number: X"])[:-40],  # truncated xref/trailer
        make_pdf(["Part Number: X"]).replace(b"/Root 1 0 R", b"/Root 9 0 R"),  # dangling root
    ],
)
def test_damaged_pdf_is_recovered_or_refused(blob):
    r = run("application/pdf", blob)  # pypdf may rebuild the xref; then the text is still data with locations
    assert r["ok"] is False or all(x["location"]["page"] == 1 for x in r["records"])


def test_pdf_flate_bomb_is_capped():
    bomb = zlib.compress(
        b"BT /F1 11 Tf 72 700 Td " + b"(A) Tj " * (40 * 1024 * 1024 // 7) + b" ET", 9
    )  # ~40 MiB inflated, ~40 KiB on disk
    pdf = build_pdf([bomb], filters="/Filter /FlateDecode ")
    t = time.monotonic()
    r = run("application/pdf", pdf, max_stream_bytes=1024 * 1024)
    assert time.monotonic() - t < 20
    assert r == {"ok": False, "error": "pdf_resource_limit"}


def test_pdf_deep_nesting_and_huge_page():
    deep = build_pdf(["[" * 200_000 + "]" * 200_000])
    assert run("application/pdf", deep)["ok"] in (True, False)
    huge = build_pdf(
        [text_at(1e9, 1e9, "Part Number: FAR-1") + " " + text_at(-1e9, -1e9, "Rev: Z")],
        mediabox="0 0 14400 14400",
    )
    r = run("application/pdf", huge)
    assert r["ok"] and {("Part Number", "FAR-1"), ("Rev", "Z")} <= {
        (x["label"], x["value"]) for x in r["records"]
    }


def test_pdf_page_text_and_page_count_limits():
    many = build_pdf([text_at(72, 700, f"Lot: L-{i}") for i in range(12)])
    r = run("application/pdf", many, max_pages=3)
    assert r["ok"] and "pdf_pages_truncated:12>3" in r["warnings"]
    assert [x["value"] for x in r["records"]] == ["L-0", "L-1", "L-2"]
    wall = build_pdf([" ".join(text_at(72, 700 - i, "X" * 200) for i in range(100))])
    assert run("application/pdf", wall, max_page_chars=5000) == {
        "ok": False,
        "error": "pdf_page_text_too_large",
    }
    r = run("application/pdf", wall, max_fragments_per_page=10)
    assert r["ok"] and "pdf_page_fragments_truncated:1" in r["warnings"]


# --- XLSX --------------------------------------------------------------------------------------------------


def zip_of(files: dict[str, bytes]) -> bytes:
    buf = io.BytesIO()
    with zipfile.ZipFile(buf, "w", zipfile.ZIP_DEFLATED) as z:
        for k, v in files.items():
            z.writestr(k, v)
    return buf.getvalue()


def test_xlsx_zip_bomb_by_ratio_and_total():
    bomb = make_xlsx([["a"], ["b"]], extra={"xl/worksheets/sheet1.xml": b"0" * (8 * 1024 * 1024)})
    assert len(bomb) < 64 * 1024
    assert run(XLSX, bomb)["error"] == "xlsx_zip_bomb"
    assert run(XLSX, make_xlsx([["a"], ["b"]]), max_total_unzipped=100)["error"] == "xlsx_zip_bomb"


def test_xlsx_lying_size_header_is_capped():
    good = make_xlsx(
        [["Part Number"], ["P-1"]], extra={"xl/worksheets/sheet1.xml": b"<x>" + b" " * 50_000 + b"</x>"}
    )
    # Rewrite the declared uncompressed size of every member to 10 bytes (local + central headers).
    data = bytearray(good)
    i = 0
    while (i := data.find(b"PK\x03\x04", i)) >= 0:
        data[i + 22 : i + 26] = struct.pack("<I", 10)
        i += 4
    i = 0
    while (i := data.find(b"PK\x01\x02", i)) >= 0:
        data[i + 24 : i + 28] = struct.pack("<I", 10)
        i += 4
    r = run(XLSX, bytes(data), max_member_bytes=1000)
    assert r == {"ok": False, "error": r.get("error")} and r["error"] in {
        "xlsx_member_too_large",
        "xlsx_bad_zip",
    }


def test_xlsx_member_count_dtd_bad_xml_and_traversal():
    many = zip_of({f"f{i}.xml": b"<a/>" for i in range(30)})
    assert run(XLSX, many, max_zip_members=20)["error"] == "xlsx_too_many_members"
    evil = make_xlsx(
        [["a"], ["b"]], extra={"xl/sharedStrings.xml": b'<!DOCTYPE x [<!ENTITY a "aaaa">]><sst/>'}
    )
    assert run(XLSX, evil)["error"] == "xlsx_dtd_refused"
    assert run(XLSX, make_xlsx([["a"]], extra={"xl/workbook.xml": b"<unclosed"}))["error"] == "xlsx_bad_xml"
    rels = b'<Relationships><Relationship Id="rId1" Target="../../../../etc/passwd"/></Relationships>'
    r = run(XLSX, make_xlsx([["a"], ["b"]], extra={"xl/_rels/workbook.xml.rels": rels}))
    assert r["ok"] and r["records"] == [] and "xlsx_sheet_target_invalid" in r["warnings"]
    assert run(XLSX, b"PK\x03\x04not a zip")["error"] == "xlsx_bad_zip"


def test_xlsx_cell_cap():
    rows = [[f"h{c}" for c in range(10)]] + [[str(r)] * 10 for r in range(20)]
    assert run(XLSX, make_xlsx(rows), max_cells=50)["error"] == "xlsx_too_many_cells"


# --- CSV, DXF, images --------------------------------------------------------------------------------------


def test_csv_bad_encoding_and_row_cap():
    assert run("text/csv", b"\xff\xfe\x00\xd8\x81")["error"] == "csv_bad_encoding"
    rows = "Lot,Qty\n" + "".join(f"L{i},1\n" for i in range(50))
    r = run("text/csv", rows.encode(), max_rows=5)
    assert len(r["records"]) == 8 and "csv_rows_truncated" in r["warnings"]


def test_dxf_hostile_inputs():
    assert run("image/vnd.dxf", b"AutoCAD Binary DXF\r\n\x1a\x00")["error"] == "dxf_binary_unsupported"
    assert run("image/vnd.dxf", b"0\nSECTION\nnotanint\nHEADER\n")["error"] == "dxf_malformed"
    assert run("image/vnd.dxf", b"0\nEOF\n" * 100, max_dxf_pairs=10)["error"] == "dxf_too_large"


def test_image_decompression_bomb_refused_before_ocr():
    png = (
        b"\x89PNG\r\n\x1a\n"
        + struct.pack(">I", 13)
        + b"IHDR"
        + struct.pack(">II", 100_000, 100_000)
        + b"\x08\x02\0\0\0"
    )
    assert run("image/png", png) == {"ok": False, "error": "image_too_large"}
    jpeg = b"\xff\xd8\xff\xc0\x00\x11\x08" + struct.pack(">HH", 50_000, 50_000) + b"\x03" + b"\0" * 12
    assert run("image/jpeg", jpeg) == {"ok": False, "error": "image_too_large"}


def test_bad_requests_and_limits_only_tighten():
    assert handle(b"not json") == {"ok": False, "error": "bad_request"}
    assert handle(b"[]") == {"ok": False, "error": "bad_request"}
    assert handle(json.dumps({"media_type": "text/csv", "data_b64": "@@"}).encode())["error"] == "bad_request"
    assert run("application/x-msdownload", b"MZ")["error"] == "unsupported_media_type"
    rows = "Lot,Qty\n" + "".join(f"L{i},1\n" for i in range(30))
    assert len(run("text/csv", rows.encode(), max_records=10**9, max_rows=4)["records"]) == 6


# --- fuzzing (hypothesis): mutated valid fixtures through the full request path -----------------------------

SEEDS = {
    "application/pdf": make_pdf(["Part Number: BRK-1", "| Lot | Qty |", "| L1 | 5 |"]),
    XLSX: make_xlsx([["Part Number", "Qty"], ["P-1", "3"]]),
    "text/csv": b"Part Number,Qty\nP-1,3\n",
    "image/vnd.dxf": b"0\nSECTION\n2\nHEADER\n9\n$ACADVER\n1\nAC1032\n0\nENDSEC\n0\nEOF\n",
}


@settings(
    max_examples=int(os.environ.get("KIZUNA_FUZZ_EXAMPLES", "250")),
    deadline=None,
    suppress_health_check=[HealthCheck.too_slow],
)
@given(
    media=st.sampled_from(sorted(SEEDS)),
    edits=st.lists(st.tuples(st.integers(0, 10**6), st.binary(min_size=0, max_size=8)), max_size=6),
    cut=st.integers(0, 10**6),
)
def test_fuzz_mutated_documents_never_crash(media, edits, cut):
    data = bytearray(SEEDS[media])
    for pos, chunk in edits:
        p = pos % (len(data) + 1)
        data[p : p + len(chunk)] = chunk
    data = bytes(data[: max(1, cut % (len(data) + 1))])
    r = run(media, data, max_pages=5, max_stream_bytes=4 * 1024 * 1024)
    assert isinstance(r["ok"], bool)
    assert r["ok"] or r["error"] != "extractor_crashed", "unexpected exception type: add a structured error"
    json.dumps(r)
