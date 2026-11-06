"""v3 layout readers: horizontal tables, label/value strips, ruling lines, hidden text layers."""

import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).parent))

from ja_corpus import cjk_pdf  # noqa: E402

from kizuna_extraction import worker  # noqa: E402
from kizuna_extraction.documents import pdf_extract  # noqa: E402
from kizuna_extraction.drawing import title_block  # noqa: E402
from kizuna_extraction.layout import Cell, Line, lines_from_fragments  # noqa: E402
from kizuna_extraction.records import Sink, records_from_lines  # noqa: E402
from kizuna_extraction.tabular import csv_records  # noqa: E402
from kizuna_extraction.worker import DEFAULT_LIMITS  # noqa: E402


def _lines(
    rows: list[list[tuple[float, str]]], y0: float = 700.0, step: float = 14.0, w: float = 9
) -> list[Line]:
    return [
        Line(1, i, [Cell(t, x, x + w * len(t)) for x, t in cells], y=y0 - step * i, size=9)
        for i, cells in enumerate(rows, start=1)
    ]


def _read(lines: list[Line]) -> set[tuple[str, str]]:
    sink = Sink(200)
    records_from_lines(lines, "pdf", sink)
    title_block(lines, sink, "pdf")
    return {(r["label"], r["value"]) for r in sink.items}


def test_horizontal_table_with_merged_measured_label_and_merged_cell():
    got = _read(
        _lines(
            [
                [(50, "検査項目"), (150, "外径"), (230, "内径")],
                [(50, "規格値(mm)"), (150, "25.00"), (230, "12.00")],
                [(50, "公差"), (150, "+0/−0.05"), (230, "+0.05/−0")],
                [(50, "規格範囲"), (150, "24.95～25.00"), (230, "12.00～12.05")],
                [(50, "測定値"), (112, "1"), (150, "24.98"), (230, "12.02")],
                [(112, "2"), (150, "24.99"), (230, "12.03")],
                [(50, "判定"), (190, "合格")],  # one cell spanning both item columns
            ],
            w=5,
        )
    )
    assert {("外径 規格値 (mm)", "25.00"), ("内径 規格値 (mm)", "12.00"), ("外径 公差", "+0/−0.05")} <= got
    assert {
        ("外径 測定値 (mm)", "24.98"),
        ("外径 測定値 (mm)", "24.99"),
        ("内径 測定値 (mm)", "12.03"),
    } <= got
    assert (
        sum(1 for lab, v in got if lab == "判定" and v == "合格") == 1
    )  # one (label, value) pair, two columns
    assert not any("～" in v for _lab, v in got)  # the range row is skipped, not read as values


def test_horizontal_table_without_item_label_and_ruled_columns():
    lines = _lines(
        [
            [(150, "全長"), (230, "幅")],
            [(50, "規格値 (mm)"), (205, "120.5"), (285, "40.0")],
            [(50, "X1"), (190, "120.48"), (285, "40.1")],
        ],
        w=5,
    )
    rules = [(140.0, 600.0, 140.0, 700.0), (220.0, 600.0, 220.0, 700.0), (300.0, 600.0, 300.0, 700.0)]
    for ln in lines:
        ln.extra["rules"] = rules
    got = _read(lines)
    # right-aligned values belong to the ruled column they sit in, not to the nearest item name
    assert {("全長 規格値 (mm)", "120.5"), ("幅 規格値 (mm)", "40.0"), ("全長 測定値 (mm)", "120.48")} <= got


def test_label_strips_above_and_below():
    above = _read(_lines([[(60, "品番"), (220, "材質")], [(66, "KZ-1"), (226, "SS400")]], step=18))
    assert {("品番", "KZ-1"), ("材質", "SS400")} <= above
    below = _read(_lines([[(66, "KZ-2"), (226, "S45C")], [(60, "品番"), (220, "材質")]], step=16))
    assert {("品番", "KZ-2"), ("材質", "S45C")} <= below
    # a lone label above a lone value (vocabulary label only)
    single = _read(_lines([[(60, "図番")], [(60, "DW-9")]], step=16))
    assert ("図番", "DW-9") in single


def test_vertical_rule_splits_tight_cells():
    frags = [(100.0, 700.0, 9.0, "120.56"), (128.0, 700.0, 9.0, "3.21")]  # gap < 1 em
    assert [c.text for c in lines_from_fragments(1, frags)[0].cells] == ["120.56 3.21"]
    ruled = lines_from_fragments(1, frags, [(127.0, 690.0, 127.0, 712.0)])
    assert [c.text for c in ruled[0].cells] == ["120.56", "3.21"]


def test_pdf_rules_and_hidden_text_layer_detected():
    items = [(60, 760, 10, "品番：KZ-1")]
    lines, _m, _w = pdf_extract(cjk_pdf([items], rules=[[(50, 700, 300, 700)]]), DEFAULT_LIMITS)
    assert lines[0].extra["rules"] == [(50.0, 700.0, 300.0, 700.0)] and "hidden_text" not in lines[0].extra
    jpg = bytes.fromhex(
        "ffd8ffe000104a46494600010100000100010000ffdb00430003020202020203020202030303030406040404040408060605"
        "0609080a0a090809090a0c0f0c0a0b0e0b09090d110d0e0f101011100a0c12131210130f101010ffdb004301030303040304"
        "08040408100b090b101010101010101010101010101010101010101010101010101010101010101010101010101010101010"
        "1010101010101010ffc00011080001000103011100021101031101ffc40014000100000000000000000000000000000009ff"
        "c40014100100000000000000000000000000000000ffc40014010100000000000000000000000000000000ffc40014110100"
        "000000000000000000000000000000ffda000c03010002110311003f0054c1ffd9"
    )
    sandwich = cjk_pdf([items], images=[jpg], invisible=True)
    lines, _m, _w = pdf_extract(sandwich, DEFAULT_LIMITS)
    assert lines[0].extra.get("hidden_text") is True


def test_hidden_text_values_checked_against_ocr(monkeypatch):
    hidden = Line(1, 1, [Cell("材質：SUS304", 60, 160)], y=760.0, size=10)
    hidden.extra.update(hidden_text=True, page_h=842.0)
    recs = [
        {
            "label": "材質",
            "value": "SUS304",
            "location": {"page": 1, "line": 1},
            "confidence": 1.0,
            "method": "pdf-text",
        },
        {
            "label": "品番",
            "value": "KZ-7",
            "location": {"page": 1, "line": 1},
            "confidence": 1.0,
            "method": "pdf-text",
        },
    ]
    top = round((842 - 760 - 10) * 200 / 72)
    ocr_line = Line(1, 1, [Cell("材質:SUS304 品番 KZ-1", 0, 400)], confidence=0.9)
    ocr_line.extra["px_bbox"] = [0, top, 400, top + 30]

    class FakeOcr:
        def __init__(self, _lang):
            pass

        def available(self):
            return True

    monkeypatch.setattr(worker, "TesseractOcr", FakeOcr)
    monkeypatch.setattr(worker, "_ocr_pdf_page", lambda *_a: ([ocr_line], []))
    worker.verify_hidden_text(b"", [hidden], recs, {"ocr": True, "ocr_language": "jpn"}, DEFAULT_LIMITS, [])
    assert recs[0]["location"]["ocr_confidence"] == 0.9  # our OCR reads the same value
    assert recs[1]["location"]["ocr_confidence"] == worker.HIDDEN_DISAGREE  # KZ-7 in the layer, KZ-1 printed
    assert all(r["method"] == "pdf-hidden-text" for r in recs)
    assert worker._agrees("+0.05/−0", "+0.05/-0") and not worker._agrees("12.08", "12.03")


def test_hidden_text_unverified_without_ocr():
    hidden = Line(1, 1, [Cell("品番：KZ-7", 60, 160)], y=760.0, size=10)
    hidden.extra["hidden_text"] = True
    recs = [
        {
            "label": "品番",
            "value": "KZ-7",
            "location": {"page": 1, "line": 1},
            "confidence": 1.0,
            "method": "x",
        }
    ]
    warnings: list[str] = []
    worker.verify_hidden_text(
        b"", [hidden], recs, {"ocr": False, "ocr_language": "jpn"}, DEFAULT_LIMITS, warnings
    )
    assert recs[0]["location"]["ocr_confidence"] == worker.HIDDEN_UNVERIFIED < 0.8
    assert warnings == ["pdf_hidden_text_unverified"]


def test_csv_preamble_with_several_pairs_per_row():
    rows = "注文書\r\n発行日,令和7年4月1日,,仕入先,東和精工\r\n\r\n"
    rows += "注文番号,品番,数量,単位\r\nPO-1,KZ-1,10,個\r\n"
    sink = Sink(100)
    csv_records(rows.encode("cp932"), DEFAULT_LIMITS, sink)
    got = {(r["label"], r["value"], r["group"]) for r in sink.items}
    assert {("発行日", "令和7年4月1日", "doc"), ("仕入先", "東和精工", "doc")} <= got


def test_records_carry_their_layout_reader():
    sink = Sink(100)
    records_from_lines(
        _lines([[(60, "品番"), (220, "材質")], [(66, "KZ-1"), (226, "SS400")]], step=18), "pdf", sink
    )
    assert {r["location"]["layout"] for r in sink.items} == {"pair"}
